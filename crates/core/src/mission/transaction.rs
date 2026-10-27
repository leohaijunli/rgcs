//! One-use Mission capability, cancellation and worker-retirement receipt.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::operation_types::MissionProtocolError;
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

/// Clones observe the same exchange. This capability does not authenticate a UI
/// caller; native composition checks the original owner/window separately.
#[derive(Clone)]
pub struct MissionTransaction {
    id: OperationId,
    generation: TransportGeneration,
    pub(super) deadline: tokio::time::Instant,
    cancellation: SerialCancellation,
    owner_cancellation: OperationCancellation,
    state: Arc<TransactionState>,
}

impl std::fmt::Debug for MissionTransaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MissionTransaction")
            .field("id", &self.id)
            .field("generation", &self.generation)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl MissionTransaction {
    pub fn new(
        id: OperationId,
        generation: TransportGeneration,
        timeout: Duration,
    ) -> Result<Self, MissionProtocolError> {
        if timeout.is_zero() {
            return Err(MissionProtocolError::TimedOut);
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(MissionProtocolError::TimedOut)?;
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
    pub fn may_have_dispatched(&self) -> bool {
        self.state.dispatched.load(Ordering::Acquire)
    }

    pub(super) fn submit(&self) -> Result<(), MissionProtocolError> {
        self.state
            .submitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| MissionProtocolError::AlreadySubmitted)
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

    pub(super) fn check(&self, connection: &ConnectionHandle) -> Result<(), MissionProtocolError> {
        if self.cancellation.is_cancelled() || self.owner_cancellation.is_cancelled() {
            return Err(MissionProtocolError::Cancelled);
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(MissionProtocolError::TimedOut);
        }
        if connection.generation() != self.generation.get() {
            return Err(MissionProtocolError::TargetChanged);
        }
        if connection.is_shutdown_requested() {
            return Err(MissionProtocolError::Stopped);
        }
        Ok(())
    }

    pub(super) async fn stopped(&self) -> MissionProtocolError {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => MissionProtocolError::Cancelled,
            _ = self.owner_cancellation.cancelled() => MissionProtocolError::Cancelled,
            _ = tokio::time::sleep_until(self.deadline) => MissionProtocolError::TimedOut,
        }
    }

    pub(super) async fn cancel_and_wait(&self) -> Result<bool, MissionProtocolError> {
        self.cancel();
        if !self.state.submitted.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut finished = self.state.finished.subscribe();
        tokio::time::timeout(OWNED_CANCEL_TIMEOUT, finished.wait_for(|done| *done))
            .await
            .map_err(|_| MissionProtocolError::TimedOut)?
            .map_err(|_| MissionProtocolError::Stopped)?;
        Ok(true)
    }
}

pub(super) struct OwnedWait(pub MissionTransaction);
impl Drop for OwnedWait {
    fn drop(&mut self) {
        if !*self.0.state.finished.borrow() {
            self.0.cancel();
        }
    }
}
