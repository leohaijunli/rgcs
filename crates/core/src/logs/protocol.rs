//! MAVLink log download protocol (issues O1 / operator request): pull the
//! SD-card log list and download log files from the FC, mirroring the
//! `params`/`mission` service pattern — a pure state machine here, the app
//! shell feeds it frames and writes the file bytes.
//!
//! PX4 v1.17 facts (mavlink_log_handler.cpp): `LOG_DATA` chunks are
//! `min(requested, sizeof(log_data.data))` = 90 bytes each, streamed from
//! the requested `ofs`; `LOG_ENTRY` carries no file name (id + UTC + size
//! only); a `LOG_REQUEST_LIST` stops on-board logging until
//! `LOG_REQUEST_END`.

use std::time::{Duration, Instant};

use ::mavlink::common::{
    MavMessage, LOG_DATA_DATA, LOG_ENTRY_DATA, LOG_REQUEST_DATA_DATA, LOG_REQUEST_END_DATA,
    LOG_REQUEST_LIST_DATA,
};

use super::error::LogError;

/// `LOG_DATA.data` capacity (MAVLink message definition).
pub const LOG_DATA_CHUNK: u32 = 90;
/// Give up on a request after this long without a reply.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1200);
/// Max re-requests of one chunk/list page before failing.
const MAX_RETRIES: u32 = 4;

/// One log on the FC's SD card.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct LogEntryInfo {
    pub id: u16,
    /// UTC seconds (0 = unknown).
    pub time_utc: u32,
    /// Size in bytes (may be approximate, PX4 docs).
    pub size: u32,
}

#[derive(Debug)]
enum State {
    Idle,
    Listing {
        received: Vec<LogEntryInfo>,
        num_logs: Option<u16>,
        sent_at: Instant,
        retries: u32,
    },
    Downloading {
        id: u16,
        size: u32,
        next_ofs: u32,
        /// Bytes collected so far (the shell persists them to disk).
        buffer: Vec<u8>,
        sent_at: Instant,
        retries: u32,
    },
}

/// What the shell should send.
#[derive(Debug, Clone, PartialEq)]
pub enum LogOutbound {
    RequestList,
    RequestData { id: u16, ofs: u32, count: u32 },
    RequestEnd,
}

/// Events surfaced to the UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LogEvent {
    /// The list request finished (PX4 answers one entry per log).
    #[serde(rename_all = "snake_case")]
    List { entries: Vec<LogEntryInfo> },
    /// Download progress for one log.
    #[serde(rename_all = "snake_case")]
    Progress { id: u16, fetched: u32, total: u32 },
    /// A download completed; `bytes` are the full log contents (the shell
    /// writes them to the save path).
    #[serde(rename_all = "snake_case")]
    Downloaded { id: u16, bytes: Vec<u8> },
}

use ts_rs::TS;

/// The MAVLink log protocol state machine (one per FC).
#[derive(Debug)]
pub struct LogProtocol {
    target_sys: u8,
    target_comp: u8,
    state: State,
}

impl LogProtocol {
    pub fn new(target_sys: u8, target_comp: u8) -> Self {
        Self {
            target_sys,
            target_comp,
            state: State::Idle,
        }
    }

    /// Whether a list/download is running.
    pub fn is_busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Start listing the FC's logs.
    pub fn begin_list(&mut self) -> Vec<MavMessage> {
        self.state = State::Listing {
            received: Vec::new(),
            num_logs: None,
            sent_at: Instant::now(),
            retries: 0,
        };
        vec![list_msg(0, u16::MAX, self.target_sys, self.target_comp)]
    }

    /// Start downloading one log (size from the list).
    pub fn begin_download(&mut self, id: u16, size: u32) -> Vec<MavMessage> {
        let count = LOG_DATA_CHUNK.min(size);
        self.state = State::Downloading {
            id,
            size,
            next_ofs: 0,
            buffer: Vec::with_capacity(size as usize),
            sent_at: Instant::now(),
            retries: 0,
        };
        vec![request_data_msg(
            id,
            0,
            count,
            self.target_sys,
            self.target_comp,
        )]
    }

    /// Cancel any running list/download.
    pub fn cancel(&mut self) -> Vec<MavMessage> {
        let was_running = self.is_busy();
        self.state = State::Idle;
        if was_running {
            vec![end_msg(self.target_sys, self.target_comp)]
        } else {
            Vec::new()
        }
    }

    /// One inbound MAVLink frame.
    ///
    /// # Errors
    ///
    /// [`LogError::NoAck`] after the retry budget, or a malformed reply.
    pub fn handle(
        &mut self,
        msg: &MavMessage,
    ) -> Result<(Vec<LogEvent>, Vec<MavMessage>), LogError> {
        match msg {
            MavMessage::LOG_ENTRY(entry) => self.on_entry(entry),
            MavMessage::LOG_DATA(data) => self.on_data(data),
            _ => Ok((Vec::new(), Vec::new())),
        }
    }

    fn on_entry(
        &mut self,
        entry: &LOG_ENTRY_DATA,
    ) -> Result<(Vec<LogEvent>, Vec<MavMessage>), LogError> {
        let State::Listing {
            received, num_logs, ..
        } = &mut self.state
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        if num_logs.is_none() {
            *num_logs = Some(entry.num_logs);
        }
        // PX4 answers "no logs" with a single LOG_ENTRY, id=0, num_logs=0:
        // that is an empty list, not an entry.
        if entry.num_logs > 0 {
            received.push(LogEntryInfo {
                id: entry.id,
                time_utc: entry.time_utc,
                size: entry.size,
            });
        }
        // Complete when every log answered (PX4: one LOG_ENTRY per log id
        // 0..num_logs-1).
        if received.len() >= usize::from(entry.num_logs) {
            let entries = std::mem::take(received);
            self.state = State::Idle;
            return Ok((
                vec![LogEvent::List { entries }],
                vec![end_msg(self.target_sys, self.target_comp)],
            ));
        }
        Ok((Vec::new(), Vec::new()))
    }

    fn on_data(
        &mut self,
        data: &LOG_DATA_DATA,
    ) -> Result<(Vec<LogEvent>, Vec<MavMessage>), LogError> {
        let State::Downloading {
            id,
            size,
            next_ofs,
            buffer,
            ..
        } = &mut self.state
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        if data.id != *id {
            return Ok((Vec::new(), Vec::new()));
        }
        // Append at the requested offset (retransmissions of an older chunk
        // overwrite their slice instead of corrupting the tail).
        let ofs = data.ofs as usize;
        if ofs > buffer.len() {
            return Err(LogError::Gap {
                id: data.id,
                ofs: data.ofs,
            });
        }
        let end = ofs + data.count as usize;
        if buffer.len() < end {
            buffer.resize(end, 0);
        }
        buffer[ofs..end].copy_from_slice(&data.data[..data.count as usize]);

        let fetched = buffer.len() as u32;
        let progress = LogEvent::Progress {
            id: *id,
            fetched,
            total: *size,
        };
        if fetched >= *size {
            let bytes = std::mem::take(buffer);
            let id = *id;
            self.state = State::Idle;
            return Ok((
                vec![progress, LogEvent::Downloaded { id, bytes }],
                vec![end_msg(self.target_sys, self.target_comp)],
            ));
        }
        *next_ofs = fetched;
        let count = LOG_DATA_CHUNK.min(*size - fetched);
        Ok((
            vec![progress],
            vec![request_data_msg(
                *id,
                fetched,
                count,
                self.target_sys,
                self.target_comp,
            )],
        ))
    }

    /// A tick with no reply: retransmit the pending request, or fail after
    /// the retry budget.
    ///
    /// # Errors
    ///
    /// [`LogError::NoAck`] when the budget is exhausted.
    pub fn tick(&mut self, now: Instant) -> Result<Vec<MavMessage>, LogError> {
        match &self.state {
            State::Listing {
                sent_at, retries, ..
            } => {
                if now.duration_since(*sent_at) < REQUEST_TIMEOUT {
                    return Ok(Vec::new());
                }
                if *retries >= MAX_RETRIES {
                    return Err(LogError::NoAck {
                        what: "log list".into(),
                    });
                }
                if let State::Listing {
                    retries, sent_at, ..
                } = &mut self.state
                {
                    *retries += 1;
                    *sent_at = now;
                }
                Ok(vec![list_msg(
                    0,
                    u16::MAX,
                    self.target_sys,
                    self.target_comp,
                )])
            }
            State::Downloading {
                id,
                size,
                next_ofs,
                sent_at,
                retries,
                ..
            } => {
                if now.duration_since(*sent_at) < REQUEST_TIMEOUT {
                    return Ok(Vec::new());
                }
                if *retries >= MAX_RETRIES {
                    return Err(LogError::NoAck {
                        what: "log data".into(),
                    });
                }
                let (id, next_ofs, size) = (*id, *next_ofs, *size);
                if let State::Downloading {
                    retries, sent_at, ..
                } = &mut self.state
                {
                    *retries += 1;
                    *sent_at = now;
                }
                let count = LOG_DATA_CHUNK.min(size - next_ofs);
                Ok(vec![request_data_msg(
                    id,
                    next_ofs,
                    count,
                    self.target_sys,
                    self.target_comp,
                )])
            }
            State::Idle => Ok(Vec::new()),
        }
    }
}

fn list_msg(start: u16, end: u16, target_sys: u8, target_comp: u8) -> MavMessage {
    MavMessage::LOG_REQUEST_LIST(LOG_REQUEST_LIST_DATA {
        start,
        end,
        target_system: target_sys,
        target_component: target_comp,
    })
}

fn request_data_msg(id: u16, ofs: u32, count: u32, target_sys: u8, target_comp: u8) -> MavMessage {
    MavMessage::LOG_REQUEST_DATA(LOG_REQUEST_DATA_DATA {
        ofs,
        count,
        id,
        target_system: target_sys,
        target_component: target_comp,
    })
}

fn end_msg(target_sys: u8, target_comp: u8) -> MavMessage {
    MavMessage::LOG_REQUEST_END(LOG_REQUEST_END_DATA {
        target_system: target_sys,
        target_component: target_comp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc_entry(id: u16, num_logs: u16, size: u32) -> MavMessage {
        MavMessage::LOG_ENTRY(LOG_ENTRY_DATA {
            time_utc: 1_700_000_000,
            size,
            id,
            num_logs,
            last_log_num: num_logs,
        })
    }

    fn fc_data(id: u16, ofs: u32, count: u8, fill: u8) -> MavMessage {
        let mut d = [0u8; 90];
        d[..count as usize].fill(fill);
        MavMessage::LOG_DATA(LOG_DATA_DATA {
            ofs,
            id,
            count,
            data: d,
        })
    }

    #[test]
    fn list_collects_all_entries_and_ends_the_request() {
        let mut p = LogProtocol::new(1, 1);
        let frames = p.begin_list();
        assert!(matches!(&frames[0], MavMessage::LOG_REQUEST_LIST(_)));

        let (events, _) = p.handle(&fc_entry(0, 3, 1024)).expect("entry 0");
        assert!(events.is_empty(), "still collecting");
        let (events, _) = p.handle(&fc_entry(1, 3, 2048)).expect("entry 1");
        assert!(events.is_empty());
        let (events, _) = p.handle(&fc_entry(2, 3, 512)).expect("entry 2");
        assert!(
            matches!(&events[0], LogEvent::List { entries } if entries.len() == 3),
            "list event after all entries"
        );
        // LOG_REQUEST_END closes the listing on the FC side.
        assert_eq!(events.len(), 1, "only the List event");
        assert!(!p.is_busy());
    }

    #[test]
    fn download_walks_the_chunks_until_size() {
        let mut p = LogProtocol::new(1, 1);
        let size = 180u32; // two 90-byte chunks
        let frames = p.begin_download(7, size);
        assert!(
            matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == 0 && d.count == 90)
        );

        let (events, frames) = p.handle(&fc_data(7, 0, 90, 1)).expect("chunk 0");
        assert!(
            matches!(&events[0], LogEvent::Progress { fetched, total, .. } if *fetched == 90 && *total == size)
        );
        assert!(
            matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == 90 && d.count == 90)
        );

        let (events, frames) = p.handle(&fc_data(7, 90, 90, 2)).expect("chunk 1");
        assert!(matches!(
            &events[1],
            LogEvent::Downloaded { id: 7, bytes } if bytes.len() == 180
        ));
        assert!(matches!(&frames[0], MavMessage::LOG_REQUEST_END(_)));
        assert!(!p.is_busy());
    }

    #[test]
    fn stale_retransmission_overwrites_its_slice() {
        let mut p = LogProtocol::new(1, 1);
        p.begin_download(7, 180);
        p.handle(&fc_data(7, 0, 90, 1)).expect("chunk 0");
        // A retransmission of chunk 0 (e.g. the FC raced our second request):
        // it must not corrupt the buffer past 90 bytes.
        let (events, _) = p.handle(&fc_data(7, 0, 90, 9)).expect("stale chunk 0");
        assert!(events
            .iter()
            .all(|e| !matches!(e, LogEvent::Downloaded { .. })));
        if let State::Downloading { buffer, .. } = &p.state {
            assert_eq!(buffer.len(), 90);
        } else {
            panic!("expected downloading state");
        }
    }

    #[test]
    fn tick_retransmits_then_fails() {
        let mut p = LogProtocol::new(1, 1);
        p.begin_list();
        let mut t = Instant::now();
        // Four retransmissions are allowed, the fifth tick fails.
        for _ in 0..MAX_RETRIES {
            t += REQUEST_TIMEOUT;
            let frames = p.tick(t).expect("retry");
            assert_eq!(frames.len(), 1);
        }
        t += REQUEST_TIMEOUT;
        assert!(matches!(p.tick(t), Err(LogError::NoAck { .. })));
    }

    #[test]
    fn zero_logs_is_a_valid_empty_list() {
        let mut p = LogProtocol::new(1, 1);
        p.begin_list();
        let (events, _) = p.handle(&fc_entry(0, 0, 0)).expect("empty reply");
        assert!(matches!(&events[0], LogEvent::List { entries } if entries.is_empty()));
    }
}
