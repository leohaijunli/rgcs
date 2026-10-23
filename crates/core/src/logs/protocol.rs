//! MAVLink log download protocol (issues O1 / operator request): pull the
//! SD-card log list and download log files from the FC, mirroring the
//! `params`/`mission` service pattern — a pure state machine here, the app
//! shell feeds it frames and writes the file bytes.
//!
//! PX4 v1.17 facts (mavlink_log_handler.cpp): `LOG_DATA` chunks are
//! `min(requested, sizeof(log_data.data))` = 90 bytes each, streamed from
//! the requested `ofs`; `LOG_ENTRY` carries no file name (id + UTC + size
//! only). `LOG_REQUEST_END` ends retrieval; logger control is independent.

use std::time::{Duration, Instant};

use ::mavlink::common::{
    MavMessage, LOG_DATA_DATA, LOG_ENTRY_DATA, LOG_ERASE_DATA, LOG_REQUEST_DATA_DATA,
    LOG_REQUEST_END_DATA, LOG_REQUEST_LIST_DATA,
};

use super::error::LogError;

/// `LOG_DATA.data` capacity (MAVLink message definition).
pub const LOG_DATA_CHUNK: u32 = 90;
/// Bytes requested per `LOG_REQUEST_DATA`. PX4 streams the whole requested
/// window as consecutive `LOG_DATA` frames instead of requiring one round trip
/// per 90-byte chunk; keep it comfortably inside the connection's 4096-frame
/// event buffer *and* below the burst-loss cliff of lossy links.
///
/// PX4 answers one request with a single burst. On a flow-controlled serial
/// link the whole window survives, but over MAVLink/UDP the burst overruns the
/// transport above ~20 KiB: measured on PX4 v1.17 SITL, windows of 18000 and
/// 20250 bytes streamed cleanly (a 2.8 MB log in ~2 s, zero retransmits) while
/// 21150+ bytes dropped frames. A dropped frame leaves a gap, so the client
/// waits for `REQUEST_TIMEOUT` before retrying from the tail — collapsing
/// throughput to ~20 KiB/s and making large logs appear to hang. 18000
/// (200 x 90) keeps a safe margin below that cliff without sacrificing speed.
pub const LOG_DOWNLOAD_WINDOW: u32 = LOG_DATA_CHUNK * 200;
// Enforce the burst-loss margin at compile time so a future bump cannot
// silently reintroduce the UDP stall this window was sized to avoid.
const _: () = assert!(LOG_DOWNLOAD_WINDOW < 20_000);
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
        /// Buffer collected so far. It is also the offset of the next request.
        buffer: Vec<u8>,
        /// Exclusive end of the currently requested window.
        request_end: u32,
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
        self.begin_list_at(Instant::now())
    }

    /// Start listing the FC's logs with an explicit clock (test seam).
    pub fn begin_list_at(&mut self, now: Instant) -> Vec<MavMessage> {
        self.state = State::Listing {
            received: Vec::new(),
            num_logs: None,
            sent_at: now,
            retries: 0,
        };
        vec![list_msg(0, u16::MAX, self.target_sys, self.target_comp)]
    }

    /// Start downloading one log (size from the list).
    pub fn begin_download(&mut self, id: u16, size: u32) -> Vec<MavMessage> {
        self.begin_download_at(id, size, Instant::now())
    }

    /// Start downloading one log with an explicit clock (test seam).
    pub fn begin_download_at(&mut self, id: u16, size: u32, now: Instant) -> Vec<MavMessage> {
        let count = LOG_DOWNLOAD_WINDOW.min(size);
        self.state = State::Downloading {
            id,
            size,
            buffer: Vec::with_capacity(count as usize),
            request_end: count,
            sent_at: now,
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

    /// Permanently delete every log file on the FC's SD card. The caller must
    /// first make sure the target is not recording; PX4 closes its current
    /// log file and removes the log directory contents.
    pub fn begin_erase(&mut self) -> Vec<MavMessage> {
        self.state = State::Idle;
        vec![MavMessage::LOG_ERASE(LOG_ERASE_DATA {
            target_system: self.target_sys,
            target_component: self.target_comp,
        })]
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

    /// One inbound MAVLink frame. `now` resets the retry window as soon as a
    /// reply makes progress, so a long healthy transfer never exhausts the
    /// retry budget.
    ///
    /// # Errors
    ///
    /// [`LogError::NoAck`] after the retry budget, or a malformed
    /// `LOG_DATA` chunk.
    pub fn handle(
        &mut self,
        msg: &MavMessage,
        now: Instant,
    ) -> Result<(Vec<LogEvent>, Vec<MavMessage>), LogError> {
        match msg {
            MavMessage::LOG_ENTRY(entry) => self.on_entry(entry, now),
            MavMessage::LOG_DATA(data) => self.on_data(data, now),
            _ => Ok((Vec::new(), Vec::new())),
        }
    }

    fn on_entry(
        &mut self,
        entry: &LOG_ENTRY_DATA,
        now: Instant,
    ) -> Result<(Vec<LogEvent>, Vec<MavMessage>), LogError> {
        let State::Listing {
            received,
            num_logs,
            sent_at,
            retries,
        } = &mut self.state
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        if entry.num_logs > 0 && entry.id >= entry.num_logs {
            return Ok((Vec::new(), Vec::new()));
        }
        if *num_logs != Some(entry.num_logs) {
            // A refreshed PX4 index can change after recording/deletion.
            received.clear();
            *num_logs = Some(entry.num_logs);
        }
        // Some implementations send num_logs=0; PX4 v1.17 itself is silent.
        // The app must verify that silent case against the SD directory.
        if entry.num_logs > 0 {
            let info = LogEntryInfo {
                id: entry.id,
                time_utc: entry.time_utc,
                size: entry.size,
            };
            if let Some(existing) = received.iter_mut().find(|info| info.id == entry.id) {
                *existing = info;
                return Ok((Vec::new(), Vec::new()));
            }
            received.push(info);
        }
        *sent_at = now;
        *retries = 0;
        // Complete when every log answered (PX4: one LOG_ENTRY per log id
        // 0..num_logs-1).
        if received.len() >= usize::from(entry.num_logs) {
            let mut entries = std::mem::take(received);
            entries.sort_by_key(|entry| entry.id);
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
        now: Instant,
    ) -> Result<(Vec<LogEvent>, Vec<MavMessage>), LogError> {
        let State::Downloading {
            id,
            size,
            buffer,
            request_end,
            sent_at,
            retries,
        } = &mut self.state
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        if data.id != *id {
            return Ok((Vec::new(), Vec::new()));
        }
        // Reject anything that cannot be a valid slice of the requested range.
        // The frame is fixed at 90 bytes, so `count` must fit; the end must
        // not run past the declared size (a malformed FC reply must never
        // panic or make us allocate past `size`).
        let count = data.count as usize;
        if count > data.data.len() || count > LOG_DATA_CHUNK as usize {
            return Err(LogError::MalformedChunk {
                id: data.id,
                ofs: data.ofs,
                count: data.count,
            });
        }
        let ofs = data.ofs as usize;
        let end = ofs.checked_add(count).ok_or(LogError::MalformedChunk {
            id: data.id,
            ofs: data.ofs,
            count: data.count,
        })?;
        // Never let a malformed reply resize the buffer past the declared size.
        if end > *size as usize {
            return Err(LogError::MalformedChunk {
                id: data.id,
                ofs: data.ofs,
                count: data.count,
            });
        }
        // Ignore the rest of a burst after a lost chunk. The timeout retries
        // from the contiguous tail once, rather than restarting PX4 for every
        // later chunk and creating a request storm.
        if ofs > buffer.len() {
            return Ok((Vec::new(), Vec::new()));
        }
        // A retransmission of an older chunk overwrites its slice; only a
        // chunk beyond the collected tail (but inside the log) is a gap.
        let made_progress = buffer.len() < end;
        if made_progress {
            buffer.resize(end, 0);
        }
        buffer[ofs..end].copy_from_slice(&data.data[..count]);
        if !made_progress {
            return Ok((Vec::new(), Vec::new()));
        }

        // Making progress re-anchors the timeout: a healthy multi-minute
        // transfer must never exhaust the retry budget (plan Task 7).
        *sent_at = now;
        *retries = 0;

        let fetched = buffer.len() as u32;
        let progress = LogEvent::Progress {
            id: *id,
            fetched,
            total: *size,
        };
        if fetched >= *size {
            // `end <= size` above means the buffer cannot exceed the size.
            debug_assert_eq!(buffer.len(), *size as usize);
            let bytes = std::mem::take(buffer);
            let id = *id;
            self.state = State::Idle;
            return Ok((
                vec![progress, LogEvent::Downloaded { id, bytes }],
                vec![end_msg(self.target_sys, self.target_comp)],
            ));
        }
        // PX4 streams the whole requested window without further requests.
        // Ask for the next window only once the current one is complete.
        if fetched < *request_end {
            return Ok((vec![progress], Vec::new()));
        }
        let count = LOG_DOWNLOAD_WINDOW.min(*size - fetched);
        *request_end = fetched.saturating_add(count);
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
                buffer,
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
                let (id, next, size) = (*id, buffer.len() as u32, *size);
                if let State::Downloading {
                    request_end,
                    retries,
                    sent_at,
                    ..
                } = &mut self.state
                {
                    *retries += 1;
                    *sent_at = now;
                    let count = LOG_DOWNLOAD_WINDOW.min(size - next);
                    *request_end = next.saturating_add(count);
                }
                let count = LOG_DOWNLOAD_WINDOW.min(size - next);
                Ok(vec![request_data_msg(
                    id,
                    next,
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
        fc_data_chunk(id, ofs, u32::from(count), fill)
    }

    fn fc_data_chunk(id: u16, ofs: u32, count: u32, fill: u8) -> MavMessage {
        let mut d = [0u8; 90];
        d[..count as usize].fill(fill);
        MavMessage::LOG_DATA(LOG_DATA_DATA {
            ofs,
            id,
            count: count as u8,
            data: d,
        })
    }

    #[test]
    fn list_collects_all_entries_and_ends_the_request() {
        let mut p = LogProtocol::new(1, 1);
        let t = Instant::now();
        let frames = p.begin_list();
        assert!(matches!(&frames[0], MavMessage::LOG_REQUEST_LIST(_)));

        let (events, _) = p.handle(&fc_entry(0, 3, 1024), t).expect("entry 0");
        assert!(events.is_empty(), "still collecting");
        let (events, _) = p.handle(&fc_entry(1, 3, 2048), t).expect("entry 1");
        assert!(events.is_empty());
        let (events, _) = p.handle(&fc_entry(2, 3, 512), t).expect("entry 2");
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
        let t = Instant::now();
        let size = 180u32; // two 90-byte chunks
        let frames = p.begin_download_at(7, size, t);
        assert!(
            matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == 0 && d.count == size)
        );

        let (events, frames) = p.handle(&fc_data(7, 0, 90, 1), t).expect("chunk 0");
        assert!(
            matches!(&events[0], LogEvent::Progress { fetched, total, .. } if *fetched == 90 && *total == size)
        );
        // The whole window was requested once; no per-chunk round trip.
        assert!(frames.is_empty(), "window still has 90 bytes left");

        let (events, frames) = p.handle(&fc_data(7, 90, 90, 2), t).expect("chunk 1");
        assert!(matches!(
            &events[1],
            LogEvent::Downloaded { id: 7, bytes } if bytes.len() == 180
        ));
        assert!(matches!(&frames[0], MavMessage::LOG_REQUEST_END(_)));
        assert!(!p.is_busy());
    }

    #[test]
    fn next_window_is_requested_only_after_the_current_one() {
        let mut p = LogProtocol::new(1, 1);
        let t = Instant::now();
        let size = LOG_DOWNLOAD_WINDOW + 180;
        let frames = p.begin_download_at(8, size, t);
        assert!(
            matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == 0 && d.count == LOG_DOWNLOAD_WINDOW)
        );

        let mut ofs = 0u32;
        while ofs < LOG_DOWNLOAD_WINDOW {
            let n = LOG_DATA_CHUNK.min(LOG_DOWNLOAD_WINDOW - ofs);
            let (_, frames) = p
                .handle(&fc_data_chunk(8, ofs, n, 1), t)
                .expect("window chunk");
            ofs += n;
            if ofs < LOG_DOWNLOAD_WINDOW {
                assert!(frames.is_empty(), "must not interrupt the in-flight window");
            } else {
                assert!(
                    matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == ofs && d.count == 180),
                    "the completed window requests exactly the remainder"
                );
            }
        }
    }

    #[test]
    fn erase_targets_the_configured_vehicle_and_is_not_busy() {
        let mut p = LogProtocol::new(7, 1);
        let frames = p.begin_erase();
        assert!(!p.is_busy());
        assert!(matches!(
            &frames.as_slice(),
            [MavMessage::LOG_ERASE(d)] if d.target_system == 7 && d.target_component == 1
        ));
    }

    #[test]
    fn stale_retransmission_overwrites_its_slice() {
        let mut p = LogProtocol::new(1, 1);
        let t = Instant::now();
        p.begin_download_at(7, 180, t);
        p.handle(&fc_data(7, 0, 90, 1), t).expect("chunk 0");
        // A retransmission of chunk 0 (e.g. the FC raced our next request):
        // it must not corrupt the buffer past 90 bytes.
        let (events, _) = p.handle(&fc_data(7, 0, 90, 9), t).expect("stale chunk 0");
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
    fn download_tick_retransmits_from_the_collected_tail() {
        let mut p = LogProtocol::new(1, 1);
        let mut t = Instant::now();
        p.begin_download_at(9, LOG_DOWNLOAD_WINDOW + 90, t);
        let (_, frames) = p.handle(&fc_data(9, 0, 90, 1), t).expect("chunk");
        assert!(frames.is_empty());
        t += REQUEST_TIMEOUT;
        let frames = p.tick(t).expect("retransmit");
        assert!(
            matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == 90 && d.count == LOG_DOWNLOAD_WINDOW)
        );
    }

    #[test]
    fn zero_logs_is_a_valid_empty_list() {
        let mut p = LogProtocol::new(1, 1);
        let t = Instant::now();
        p.begin_list();
        let (events, _) = p.handle(&fc_entry(0, 0, 0), t).expect("empty reply");
        assert!(matches!(&events[0], LogEvent::List { entries } if entries.is_empty()));
    }

    /// A long healthy transfer must survive many ticks that would otherwise
    /// spend the retry budget: every accepted chunk re-anchors the timeout.
    #[test]
    fn long_healthy_transfer_never_exhausts_the_retry_budget() {
        let size = LOG_DOWNLOAD_WINDOW + 180;
        let mut p = LogProtocol::new(1, 1);
        let start = Instant::now();
        let mut now = start;
        p.begin_download_at(9, size, now);

        let mut expected = Vec::with_capacity(size as usize);
        let mut downloaded = None;
        let mut ofs = 0u32;
        while ofs < size {
            let chunk = LOG_DATA_CHUNK.min(size - ofs);
            let fill = (ofs % 251) as u8;
            expected.extend(std::iter::repeat_n(fill, chunk as usize));

            // A slow link: ticks fire between chunks, but every chunk is
            // progress and resets the retry budget. The window request is
            // not re-sent on every tick unless the link actually stalls.
            now += REQUEST_TIMEOUT / 4;

            let mut data = [0u8; 90];
            data[..chunk as usize].fill(fill);
            let (events, _) = p
                .handle(
                    &MavMessage::LOG_DATA(LOG_DATA_DATA {
                        ofs,
                        id: 9,
                        count: chunk as u8,
                        data,
                    }),
                    now,
                )
                .expect("healthy chunk");
            for event in events {
                if let LogEvent::Downloaded { bytes, .. } = event {
                    downloaded = Some(bytes);
                }
            }
            ofs += chunk;
        }

        assert!(now.duration_since(start) >= Duration::from_secs(10));
        let bytes = downloaded.expect("final Downloaded event");
        assert_eq!(bytes.len(), size as usize);
        assert_eq!(bytes, expected, "byte-exact transfer");
        assert!(!p.is_busy());
    }

    #[test]
    fn malformed_chunk_count_is_rejected_without_panic() {
        let mut p = LogProtocol::new(1, 1);
        let t = Instant::now();
        p.begin_download_at(3, 180, t);
        let bad = MavMessage::LOG_DATA(LOG_DATA_DATA {
            ofs: 0,
            id: 3,
            count: 200,
            data: [7u8; 90],
        });
        assert!(matches!(
            p.handle(&bad, t),
            Err(LogError::MalformedChunk { count: 200, .. })
        ));
        // The state survives a malformed frame and still accepts a valid one.
        let (events, _) = p.handle(&fc_data(3, 0, 90, 1), t).expect("valid chunk");
        assert!(matches!(&events[0], LogEvent::Progress { fetched: 90, .. }));
    }

    #[test]
    fn chunk_past_the_declared_size_is_rejected() {
        let mut p = LogProtocol::new(1, 1);
        let t = Instant::now();
        p.begin_download_at(4, 100, t);
        let bad = MavMessage::LOG_DATA(LOG_DATA_DATA {
            ofs: 50,
            id: 4,
            count: 90,
            data: [1u8; 90],
        });
        assert!(matches!(
            p.handle(&bad, t),
            Err(LogError::MalformedChunk { ofs: 50, .. })
        ));
        // A lost frame is recovered from the collected tail, not failed.
        let gap = MavMessage::LOG_DATA(LOG_DATA_DATA {
            ofs: 0,
            id: 4,
            count: 90,
            data: [1u8; 90],
        });
        assert!(p.handle(&gap, t).is_ok());
        let beyond = MavMessage::LOG_DATA(LOG_DATA_DATA {
            ofs: 95,
            id: 4,
            count: 5,
            data: [1u8; 90],
        });
        let (events, frames) = p.handle(&beyond, t).expect("ignore burst after gap");
        assert!(events.is_empty());
        assert!(frames.is_empty());
        let frames = p.tick(t + REQUEST_TIMEOUT).expect("re-request tail");
        assert!(
            matches!(&frames[0], MavMessage::LOG_REQUEST_DATA(d) if d.ofs == 90 && d.count == 10)
        );
    }

    #[test]
    fn repeated_list_entries_do_not_complete_a_partial_list() {
        let mut p = LogProtocol::new(1, 1);
        let now = Instant::now();
        p.begin_list_at(now);
        for _ in 0..5 {
            let (events, _) = p.handle(&fc_entry(0, 3, 1024), now).expect("duplicate");
            assert!(events.is_empty());
        }
        p.handle(&fc_entry(2, 3, 2048), now).expect("entry 2");
        let (events, _) = p.handle(&fc_entry(1, 3, 4096), now).expect("entry 1");
        let LogEvent::List { entries } = &events[0] else { panic!("list") };
        assert_eq!(entries.iter().map(|entry| entry.id).collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn px4_full_sized_chunks_complete_every_window_byte_exactly() {
        let size = LOG_DOWNLOAD_WINDOW * 3 + 47;
        let now = Instant::now();
        let mut p = LogProtocol::new(250, 1);
        let mut frames = p.begin_download_at(0, size, now);
        let mut requests = 0;
        let mut downloaded = None;
        while let Some(frame) = frames.pop() {
            let MavMessage::LOG_REQUEST_DATA(request) = frame else { continue };
            requests += 1;
            let mut offset = request.ofs;
            while offset < request.ofs + request.count {
                // PX4 reads 90 bytes except at EOF, not at the request boundary.
                let count = LOG_DATA_CHUNK.min(size - offset);
                let (events, next) = p.handle(&fc_data_chunk(0, offset, count, 42), now).expect("PX4 chunk");
                frames.extend(next);
                for event in events {
                    if let LogEvent::Downloaded { bytes, .. } = event {
                        downloaded = Some(bytes);
                    }
                }
                offset += count;
            }
        }
        assert_eq!(requests, 4);
        assert_eq!(downloaded.expect("completed"), vec![42; size as usize]);
    }

    #[test]
    fn duplicate_or_future_chunks_cannot_hide_a_stalled_download() {
        let mut p = LogProtocol::new(1, 1);
        let mut now = Instant::now();
        p.begin_download_at(0, 900, now);
        p.handle(&fc_data(0, 0, 90, 1), now).expect("progress");
        for _ in 0..MAX_RETRIES {
            now += REQUEST_TIMEOUT;
            p.handle(&fc_data(0, 0, 90, 1), now).expect("duplicate");
            p.handle(&fc_data(0, 180, 90, 1), now).expect("future chunk");
            assert_eq!(p.tick(now).expect("retry").len(), 1);
        }
        now += REQUEST_TIMEOUT;
        assert!(matches!(p.tick(now), Err(LogError::NoAck { .. })));
    }
}
