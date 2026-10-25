//! Parameter session state machine (parameters plan §4.2) — pure logic, the
//! service shell feeds it frames and sends the frames it returns.
//!
//! Pull: `PARAM_REQUEST_LIST` → streamed `PARAM_VALUE` into a received
//! bitmap; when the stream goes quiet (the shell decides — a tick with no
//! new entries) missing indexes are re-requested one by one (`PARAM_REQUEST_READ`),
//! each capped at a retry count; duplicates and reordering are tolerated at
//! any time. Cancel stops everything immediately.
//!
//! Set: `PARAM_SET` → wait for the echo `PARAM_VALUE`; retry on timeout;
//! an echo that differs from the request is "clamped/rejected" and the echo
//! value wins (the cache must match reality).

use std::collections::HashMap;
use std::time::Duration;

use super::codec::{self, IntEncoding};
use super::model::{ParamEntry, ParamType, ParamValue};

/// One inbound `PARAM_VALUE` (decoded by the shell).
#[derive(Debug, Clone)]
pub struct IncomingParam {
    pub index: u16,
    pub count: u16,
    pub name: String,
    pub raw_value: f32,
    pub ptype: ParamType,
}

/// What the shell should send.
#[derive(Debug, Clone, PartialEq)]
pub enum Outbound {
    RequestList,
    RequestRead {
        index: u16,
    },
    RequestReadByName {
        name: String,
    },
    /// `PARAM_SET` for one parameter.
    Set {
        name: String,
        value: f32,
        ptype: ParamType,
    },
}

/// Final tally of one full parameter pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullReport {
    pub received: u16,
    pub total: u16,
}

impl PullReport {
    /// The FC's complete parameter inventory was received. An empty inventory
    /// is not usable as a capability proof.
    pub fn is_complete(self) -> bool {
        self.total > 0 && self.received >= self.total
    }
}

/// Pull lifecycle events, batched for the UI channel.
#[derive(Debug, Clone, PartialEq)]
pub enum PullUpdate {
    /// New/updated entries since the last batch (~100 per batch).
    Batch(Vec<ParamEntry>),
    /// Progress: distinct indexes received so far.
    Progress { received: u16, total: u16 },
    /// Complete: received may be < total (missing listed); the session
    /// returns to Idle and the caller decides whether to retry.
    Done { received: u16, total: u16 },
}

/// Set transaction outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum SetOutcome {
    /// Echo confirms the exact requested value.
    Confirmed(ParamValue),
    /// Echo differs: the FC clamped/rejected; the echo value is what the
    /// FC actually holds now.
    Clamped {
        requested: ParamValue,
        actual: ParamValue,
    },
    /// Retries exhausted.
    Timeout,
    TypeMismatch {
        expected: ParamType,
    },
}

/// Retry budgets (plan §4.2; N retries per missing index).
pub const MAX_READ_RETRIES: u32 = 3;
/// Max re-requested indexes per fill round before declaring Done.
pub const MAX_FILL_ROUNDS: u32 = 3;
/// Quiet period on the pull stream that triggers a fill round (plan: 2 s).
pub const PULL_QUIET: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct ParamSession {
    encoding: IntEncoding,
    /// Pull state.
    total: Option<u16>,
    received: HashMap<u16, ParamEntry>,
    fill_round: u32,
    fill_queue: Vec<u16>,
    fill_retry: HashMap<u16, u32>,
    /// Active set transaction (one at a time, §4.2).
    pending_set: Option<PendingSet>,
    /// Active single read (by name, operator lazy-load request).
    pending_read: Option<PendingRead>,
    /// `true` while a pull is active (between RequestList and Done).
    pulling: bool,
    /// Outcome of the last set echo, consumed by the shell.
    last_set_outcome: Option<(String, SetOutcome)>,
}

#[derive(Debug)]
struct PendingRead {
    name: String,
    retries: u32,
}

#[derive(Debug)]
struct PendingSet {
    name: String,
    value: ParamValue,
    ptype: ParamType,
    retries: u32,
}

impl Default for ParamSession {
    fn default() -> Self {
        Self::new(IntEncoding::default())
    }
}

impl ParamSession {
    pub fn new(encoding: IntEncoding) -> Self {
        Self {
            encoding,
            total: None,
            received: HashMap::new(),
            fill_round: 0,
            fill_queue: Vec::new(),
            fill_retry: HashMap::new(),
            pending_set: None,
            pending_read: None,
            pulling: false,
            last_set_outcome: None,
        }
    }

    pub fn entries(&self) -> Vec<ParamEntry> {
        let mut out: Vec<ParamEntry> = self.received.values().cloned().collect();
        out.sort_by_key(|e| e.index);
        out
    }

    pub fn set_encoding(&mut self, encoding: IntEncoding) {
        if !self.is_busy() {
            self.encoding = encoding;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.pulling || self.pending_set.is_some() || self.pending_read.is_some()
    }

    // ---- Pull ------------------------------------------------------------

    /// Start a full pull. `Err` if a transaction is already running.
    pub fn start_pull(&mut self) -> Result<Vec<Outbound>, &'static str> {
        if self.is_busy() {
            return Err("a parameter transaction is already running");
        }
        self.received.clear();
        self.total = None;
        self.fill_round = 0;
        self.fill_queue.clear();
        self.fill_retry.clear();
        self.pulling = true;
        Ok(vec![Outbound::RequestList])
    }

    /// One `PARAM_VALUE` arrived. Returns the batch to push (when the caller
    /// should flush) and progress.
    pub fn on_param_value(
        &mut self,
        p: IncomingParam,
    ) -> (Option<ParamEntry>, Option<PullUpdate>, Option<Outbound>) {
        // A single read (by name) resolves on the first matching reply.
        if let Some(pending) = &self.pending_read {
            if pending.name == p.name {
                self.pending_read = None;
                let entry = ParamEntry {
                    name: p.name,
                    index: p.index,
                    value: codec::decode(p.raw_value, p.ptype, self.encoding),
                    ptype: p.ptype,
                };
                self.received.insert(p.index, entry.clone());
                return (Some(entry), None, None);
            }
        }
        // A set echo also arrives as PARAM_VALUE; route it first.
        if let Some(pending) = &self.pending_set {
            if pending.name == p.name {
                let pending = self.pending_set.take().expect("checked");
                let actual = codec::decode(p.raw_value, p.ptype, self.encoding);
                let outcome = if pending.ptype != p.ptype {
                    SetOutcome::TypeMismatch {
                        expected: pending.ptype,
                    }
                } else if pending.value.same_as(actual) {
                    SetOutcome::Confirmed(actual)
                } else {
                    SetOutcome::Clamped {
                        requested: pending.value,
                        actual,
                    }
                };
                let entry = ParamEntry {
                    name: p.name.clone(),
                    index: p.index,
                    value: actual,
                    ptype: p.ptype,
                };
                self.received.insert(p.index, entry.clone());
                // Not part of the pull stream: report the set result via the
                // shell (the shell reads `take_set_result` after this call).
                self.last_set_outcome = Some((pending.name.clone(), outcome));
                return (Some(entry), None, None);
            }
        }

        if !self.pulling {
            // Unsolicited broadcast (e.g. another GCS changed a param):
            // store it and hand it to the shell (external change). Sentinel
            // and out-of-table frames are still not parameters.
            if p.name == "_HASH_CHECK" || p.index >= p.count {
                return (None, None, None);
            }
            let entry = ParamEntry {
                name: p.name,
                index: p.index,
                value: codec::decode(p.raw_value, p.ptype, self.encoding),
                ptype: p.ptype,
            };
            self.received.insert(p.index, entry.clone());
            return (Some(entry), None, None);
        }

        // Update the declared total whenever it appears.
        if let Some(total) = self.total {
            if p.count != total {
                // A different total mid-pull is unexpected; trust the last
                // word (the FC is authoritative).
            }
        }
        self.total = Some(p.count);
        if p.index >= p.count || p.name == "_HASH_CHECK" {
            return (None, None, None);
        }
        let entry = ParamEntry {
            name: p.name,
            index: p.index,
            value: codec::decode(p.raw_value, p.ptype, self.encoding),
            ptype: p.ptype,
        };
        self.received.insert(p.index, entry.clone());
        let received = self.received.len() as u16;
        let total = p.count;
        if received >= total {
            // Complete: no fill round needed.
            self.pulling = false;
            self.fill_queue.clear();
            return (
                Some(entry),
                Some(PullUpdate::Done { received, total }),
                None,
            );
        }
        (
            Some(entry),
            Some(PullUpdate::Progress { received, total }),
            None,
        )
    }

    /// A pull tick with no new entries for `PULL_QUIET`. Three cases:
    ///
    /// 1. Not even the first `PARAM_VALUE` (and thus the total) arrived —
    ///    the `PARAM_REQUEST_LIST` itself was lost (a `udpin` link drops
    ///    sends until its first inbound frame teaches it the peer): re-issue
    ///    it, bounded by the round budget so a dead link still ends in Done.
    /// 2. Fill round budget exhausted: declare Done.
    /// 3. Otherwise: queue the missing indexes (the shell drains them one
    ///    per tick).
    pub fn on_pull_timeout(&mut self) -> (Option<PullUpdate>, Option<Outbound>) {
        if !self.pulling {
            return (None, None);
        }
        if self.total.is_none() {
            self.fill_round += 1;
            if self.fill_round >= MAX_FILL_ROUNDS {
                self.pulling = false;
                return (
                    Some(PullUpdate::Done {
                        received: self.received.len() as u16,
                        total: 0,
                    }),
                    None,
                );
            }
            return (None, Some(Outbound::RequestList));
        }
        if self.fill_round >= MAX_FILL_ROUNDS {
            self.pulling = false;
            return (
                Some(PullUpdate::Done {
                    received: self.received.len() as u16,
                    total: self.total.unwrap_or(0),
                }),
                None,
            );
        }
        self.fill_round += 1;
        let total = self.total.unwrap_or(0);
        self.fill_queue.clear();
        for index in 0..total {
            if !self.received.contains_key(&index) {
                self.fill_queue.push(index);
            }
        }
        // The shell drains `next_fill_request` one tick at a time.
        (None, None)
    }

    /// Pop the next fill request (rate-limit friendly: one per call).
    pub fn next_fill_request(&mut self) -> Option<Outbound> {
        if !self.pulling {
            return None;
        }
        loop {
            let index = self.fill_queue.pop()?;
            let retries = self.fill_retry.get(&index).copied().unwrap_or(0);
            if retries >= MAX_READ_RETRIES {
                continue; // give up on this index
            }
            self.fill_retry.insert(index, retries + 1);
            return Some(Outbound::RequestRead { index });
        }
    }

    /// Whether the pull has finished (Done or cancelled).
    pub fn pull_active(&self) -> bool {
        self.pulling
    }

    /// Cancel whatever is running (plan T2: cancel stops requests).
    pub fn cancel(&mut self) {
        self.pulling = false;
        self.fill_queue.clear();
        self.pending_set = None;
        self.pending_read = None;
    }

    // ---- Set -------------------------------------------------------------

    /// Begin a single-parameter read by name (lazy load). `Err` when
    /// another transaction is running.
    pub fn start_read(&mut self, name: &str) -> Result<Vec<Outbound>, &'static str> {
        if self.is_busy() {
            return Err("a parameter transaction is already running");
        }
        self.pending_read = Some(PendingRead {
            name: name.to_string(),
            retries: 0,
        });
        Ok(vec![Outbound::RequestReadByName {
            name: name.to_string(),
        }])
    }

    /// Read timeout: retry up to the budget, then give up silently (the
    /// shell resolves its waiter with an error).
    pub fn on_read_timeout(&mut self) -> bool {
        let Some(pending) = self.pending_read.as_mut() else {
            return false;
        };
        if pending.retries >= MAX_READ_RETRIES {
            self.pending_read = None;
            return false; // gave up
        }
        pending.retries += 1;
        true // caller re-sends the request
    }

    /// The name being read, if any (for the shell's retry frame).
    pub fn reading_name(&self) -> Option<String> {
        self.pending_read.as_ref().map(|r| r.name.clone())
    }

    /// Begin a set transaction. `Err` if another transaction is running.
    pub fn start_set(
        &mut self,
        name: &str,
        value: ParamValue,
        ptype: ParamType,
    ) -> Result<Vec<Outbound>, &'static str> {
        if self.is_busy() {
            return Err("a parameter transaction is already running");
        }
        self.last_set_outcome = None;
        let raw = codec::encode(value, self.encoding);
        self.pending_set = Some(PendingSet {
            name: name.to_string(),
            value,
            ptype,
            retries: 0,
        });
        Ok(vec![Outbound::Set {
            name: name.to_string(),
            value: raw,
            ptype,
        }])
    }

    /// Set timeout: retry up to the budget, then report Timeout.
    pub fn on_set_timeout(&mut self) -> Option<Outbound> {
        let pending = self.pending_set.as_mut()?;
        if pending.retries >= MAX_READ_RETRIES {
            let name = pending.name.clone();
            self.pending_set = None;
            self.last_set_outcome = Some((name, SetOutcome::Timeout));
            return None;
        }
        pending.retries += 1;
        Some(Outbound::Set {
            name: pending.name.clone(),
            value: codec::encode(pending.value, self.encoding),
            ptype: pending.ptype,
        })
    }

    /// The last set outcome, if any (consumed by the shell).
    pub fn take_set_outcome(&mut self) -> Option<(String, SetOutcome)> {
        self.last_set_outcome.take()
    }

    /// A param name lookup helper for the shell (fills use names when the
    /// FC prefers it).
    pub fn name_of(&self, index: u16) -> Option<String> {
        self.received.get(&index).map(|e| e.name.clone())
    }
}

#[cfg(test)]
mod pull_reissue_tests {
    use super::*;

    fn session() -> ParamSession {
        ParamSession::new(IntEncoding::ByteConcat)
    }

    #[test]
    fn lost_list_request_is_reissued_until_budget_ends() {
        let mut s = session();
        s.start_pull().expect("idle");
        // No PARAM_VALUE ever arrives (the list request was dropped by a
        // link still learning its peer): every quiet window re-issues
        // PARAM_REQUEST_LIST until the round budget turns it into Done.
        for expect_done in [false, false, true] {
            let (update, resend) = s.on_pull_timeout();
            if expect_done {
                assert!(matches!(
                    update,
                    Some(PullUpdate::Done {
                        received: 0,
                        total: 0
                    })
                ));
                assert!(resend.is_none());
            } else {
                assert_eq!(update, None);
                assert_eq!(resend, Some(Outbound::RequestList));
            }
        }
        assert!(!s.pull_active());
    }

    #[test]
    fn quiet_window_with_known_total_builds_a_fill_round() {
        let mut s = session();
        s.start_pull().expect("idle");
        // First entry teaches the total; entries 1 and 2 stay missing.
        let _ = s.on_param_value(IncomingParam {
            index: 0,
            count: 3,
            name: "P_0000".into(),
            raw_value: 0.0,
            ptype: ParamType::I32,
        });
        let (update, resend) = s.on_pull_timeout();
        assert_eq!(update, None);
        assert!(resend.is_none());
        // The fill queue holds the missing indexes, drained one per call.
        let mut requested = Vec::new();
        while let Some(out) = s.next_fill_request() {
            if let Outbound::RequestRead { index } = out {
                requested.push(index);
            }
        }
        assert_eq!(requested, vec![2, 1]);
    }

    #[test]
    fn a_late_first_answer_stops_the_reissue() {
        let mut s = session();
        s.start_pull().expect("idle");
        let (_, resend) = s.on_pull_timeout();
        assert_eq!(resend, Some(Outbound::RequestList));
        // The re-issued request gets heard: the answer sets the total and
        // the next quiet window fills instead of re-requesting the list.
        let _ = s.on_param_value(IncomingParam {
            index: 0,
            count: 1,
            name: "P_0000".into(),
            raw_value: 1.0,
            ptype: ParamType::I32,
        });
        let (update, resend) = s.on_pull_timeout();
        assert_eq!(update, None);
        assert!(resend.is_none());
    }

    #[test]
    fn hash_check_sentinel_is_not_a_parameter_or_progress() {
        let mut s = session();
        s.start_pull().expect("idle");
        // PX4 emits `_HASH_CHECK` with a special index outside the real
        // table: it must not count toward `received` or satisfy completion.
        let (entry, update, _) = s.on_param_value(IncomingParam {
            index: 65_535,
            count: 2,
            name: "_HASH_CHECK".into(),
            raw_value: 1.0,
            ptype: ParamType::I32,
        });
        assert!(entry.is_none());
        assert!(update.is_none());
        assert_eq!(s.entries().len(), 0, "hash sentinel is not stored");
        // The real parameter still completes the (2-entry) pull.
        let (_, update, _) = s.on_param_value(IncomingParam {
            index: 0,
            count: 2,
            name: "COM_MOT_TEST_EN".into(),
            raw_value: f32::from_bits(1),
            ptype: ParamType::I32,
        });
        assert!(matches!(
            update,
            Some(PullUpdate::Progress {
                received: 1,
                total: 2
            })
        ));
    }

    #[test]
    fn out_of_range_index_is_dropped() {
        let mut s = session();
        let (entry, update, _) = s.on_param_value(IncomingParam {
            index: 5,
            count: 2,
            name: "BAD".into(),
            raw_value: 0.0,
            ptype: ParamType::I32,
        });
        assert!(entry.is_none());
        assert!(update.is_none());
    }

    #[test]
    fn set_echo_with_a_different_type_is_a_mismatch_not_a_confirmation() {
        let mut s = session();
        s.start_set("COM_MOT_TEST_EN", ParamValue::Int(1), ParamType::I32)
            .expect("idle");
        // A buggy/foreign FC echoes REAL32 for an INT32 request: the echo
        // value must never be accepted as the requested integer.
        let (entry, _, _) = s.on_param_value(IncomingParam {
            index: 0,
            count: 1,
            name: "COM_MOT_TEST_EN".into(),
            raw_value: 1.0,
            ptype: ParamType::Real32,
        });
        assert!(entry.is_some(), "the observed value is still cached");
        let (name, outcome) = s.take_set_outcome().expect("set resolved");
        assert_eq!(name, "COM_MOT_TEST_EN");
        assert_eq!(
            outcome,
            SetOutcome::TypeMismatch {
                expected: ParamType::I32
            }
        );
    }

    #[test]
    fn encoding_cannot_change_mid_transaction() {
        let mut s = session();
        s.start_set("P", ParamValue::Int(1), ParamType::I32)
            .expect("idle");
        s.set_encoding(IntEncoding::Cast);
        assert_eq!(s.encoding, IntEncoding::ByteConcat);
        s.on_set_timeout(); // exhaust
        s.on_set_timeout();
        s.on_set_timeout();
        s.on_set_timeout(); // budget exhausted: transaction over
        assert!(!s.is_busy());
        s.set_encoding(IntEncoding::Cast);
        assert_eq!(s.encoding, IntEncoding::Cast);
    }
}
