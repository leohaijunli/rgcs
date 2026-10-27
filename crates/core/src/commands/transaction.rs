//! Identity, cancellation and worker-retirement evidence for one owned command.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::CommandError;
use crate::devices::SerialCancellation;
use crate::mavlink::ConnectionHandle;
use crate::runtime::operations::OperationCancellation;
use crate::runtime::{OperationId, TransportGeneration};

pub const OWNED_CANCEL_TIMEOUT: Duration = Duration::from_secs(1);

struct TransactionState {
    submitted: AtomicBool,
    dispatched: AtomicBool,
    finished: watch::Sender<bool>,
}

/// One use of the existing command worker. Clones observe/cancel the same work;
/// neither a transaction ID nor this protocol capability authenticates a UI client.
#[derive(Clone)]
pub struct CommandTransaction {
    id: OperationId,
    generation: TransportGeneration,
    pub(super) deadline: tokio::time::Instant,
    cancellation: SerialCancellation,
    owner_cancellation: OperationCancellation,
    state: Arc<TransactionState>,
}

impl std::fmt::Debug for CommandTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandTransaction")
            .field("id", &self.id)
            .field("generation", &self.generation)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl CommandTransaction {
    pub fn new(
        id: OperationId,
        generation: TransportGeneration,
        timeout: Duration,
    ) -> Result<Self, CommandError> {
        if timeout.is_zero() {
            return Err(CommandError::TimedOut);
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(CommandError::TimedOut)?;
        Ok(Self {
            id,
            generation,
            deadline,
            cancellation: SerialCancellation::new(),
            owner_cancellation: OperationCancellation::default(),
            state: Arc::new(TransactionState {
                submitted: AtomicBool::new(false),
                dispatched: AtomicBool::new(false),
                finished: watch::channel(false).0,
            }),
        })
    }

    /// Bind native owner expiry directly to queue/write/retry admission, including
    /// the interval between the adapter's last check and a transport write.
    pub fn with_owner_cancellation(mut self, cancellation: OperationCancellation) -> Self {
        self.owner_cancellation = cancellation;
        self
    }

    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn generation(&self) -> TransportGeneration {
        self.generation
    }

    /// Conservative: entering a cancellable transport write may already have
    /// affected the FC. A cancelled receipt cannot prove the bytes were unsent.
    pub fn may_have_dispatched(&self) -> bool {
        self.state.dispatched.load(Ordering::Acquire)
    }

    pub(super) fn submit(&self) -> Result<(), CommandError> {
        self.state
            .submitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| CommandError::AlreadySubmitted)
    }

    pub(super) fn mark_dispatched(&self) {
        self.state.dispatched.store(true, Ordering::Release);
    }

    pub(super) fn finish(&self) {
        self.state.finished.send_replace(true);
    }

    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(super) fn check(&self, handle: &ConnectionHandle) -> Result<(), CommandError> {
        if self.cancellation.is_cancelled() || self.owner_cancellation.is_cancelled() {
            return Err(CommandError::Cancelled);
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(CommandError::TimedOut);
        }
        if handle.generation() != self.generation.get() {
            return Err(CommandError::TargetChanged);
        }
        if handle.is_shutdown_requested() {
            return Err(CommandError::LinkLost);
        }
        Ok(())
    }

    pub(super) async fn stopped(&self) -> CommandError {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => CommandError::Cancelled,
            _ = self.owner_cancellation.cancelled() => CommandError::Cancelled,
            _ = tokio::time::sleep_until(self.deadline) => CommandError::TimedOut,
        }
    }

    pub(super) async fn cancel_and_wait(&self) -> Result<bool, CommandError> {
        self.cancel();
        if !self.state.submitted.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut finished = self.state.finished.subscribe();
        tokio::time::timeout(OWNED_CANCEL_TIMEOUT, finished.wait_for(|done| *done))
            .await
            .map_err(|_| CommandError::TimedOut)?
            .map_err(|_| CommandError::ServiceStopped)?;
        Ok(true)
    }
}

/// A dropped caller revokes queued work and retransmission; the worker-owned
/// request separately confirms when it has actually left the protocol slot.
pub(super) struct OwnedWait(pub CommandTransaction);

impl Drop for OwnedWait {
    fn drop(&mut self) {
        if !*self.0.state.finished.borrow() {
            self.0.cancel();
        }
    }
}
