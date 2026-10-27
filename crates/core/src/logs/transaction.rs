use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

use super::operation_types::LogProtocolError;
use crate::devices::SerialCancellation;
use crate::mavlink::ConnectionHandle;
use crate::runtime::operations::OperationCancellation;
use crate::runtime::{OperationId, TransportGeneration};

pub const OWNED_CANCEL_TIMEOUT: Duration = Duration::from_secs(1);

struct State {
    submitted: AtomicBool,
    dispatched: AtomicBool,
    finished: watch::Sender<Option<Result<(), LogProtocolError>>>,
}

#[derive(Clone)]
pub struct LogTransaction {
    id: OperationId,
    generation: TransportGeneration,
    pub(super) deadline: tokio::time::Instant,
    cancellation: SerialCancellation,
    owner_cancellation: OperationCancellation,
    state: Arc<State>,
}

impl std::fmt::Debug for LogTransaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogTransaction")
            .field("id", &self.id)
            .field("generation", &self.generation)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl LogTransaction {
    pub fn new(
        id: OperationId,
        generation: TransportGeneration,
        timeout: Duration,
    ) -> Result<Self, LogProtocolError> {
        if timeout.is_zero() {
            return Err(LogProtocolError::TimedOut);
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(LogProtocolError::TimedOut)?;
        Ok(Self {
            id,
            generation,
            deadline,
            cancellation: SerialCancellation::new(),
            owner_cancellation: OperationCancellation::default(),
            state: Arc::new(State {
                submitted: AtomicBool::new(false),
                dispatched: AtomicBool::new(false),
                finished: watch::channel(None).0,
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
    pub(super) fn submit(&self) -> Result<(), LogProtocolError> {
        self.state
            .submitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LogProtocolError::AlreadySubmitted)
    }
    pub(super) fn mark_dispatched(&self) {
        self.state.dispatched.store(true, Ordering::Release);
    }
    pub(super) fn finish(&self) {
        // Drop/abort is only a queue-retirement receipt. After dispatch, only
        // the driver's explicit END/quiet receipt may confirm cleanup.
        self.finish_cleanup(if self.may_have_dispatched() {
            Err(LogProtocolError::CleanupTimeout)
        } else {
            Ok(())
        });
    }
    pub(super) fn finish_cleanup(&self, cleanup: Result<(), LogProtocolError>) {
        self.state.finished.send_if_modified(|finished| {
            if finished.is_some() {
                false
            } else {
                *finished = Some(cleanup);
                true
            }
        });
    }
    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub(super) fn check(&self, handle: &ConnectionHandle) -> Result<(), LogProtocolError> {
        if self.cancellation.is_cancelled() || self.owner_cancellation.is_cancelled() {
            return Err(LogProtocolError::Cancelled);
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(LogProtocolError::TimedOut);
        }
        if handle.generation() != self.generation.get() {
            return Err(LogProtocolError::TargetChanged);
        }
        if handle.is_shutdown_requested() {
            return Err(LogProtocolError::Stopped);
        }
        Ok(())
    }
    pub(super) async fn stopped(&self) -> LogProtocolError {
        tokio::select! { biased;
            _ = self.cancellation.cancelled() => LogProtocolError::Cancelled,
            _ = self.owner_cancellation.cancelled() => LogProtocolError::Cancelled,
            _ = tokio::time::sleep_until(self.deadline) => LogProtocolError::TimedOut,
        }
    }
    pub(super) async fn cancel_and_wait(&self) -> Result<bool, LogProtocolError> {
        self.cancel();
        if !self.state.submitted.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut finished = self.state.finished.subscribe();
        let receipt =
            tokio::time::timeout(OWNED_CANCEL_TIMEOUT, finished.wait_for(Option::is_some))
                .await
                .map_err(|_| LogProtocolError::TimedOut)?
                .map_err(|_| LogProtocolError::Stopped)?;
        receipt.as_ref().ok_or(LogProtocolError::Stopped)?.clone()?;
        Ok(true)
    }
}

pub(super) struct OwnedWait(pub LogTransaction);
impl Drop for OwnedWait {
    fn drop(&mut self) {
        if self.0.state.finished.borrow().is_none() {
            self.0.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropped_dispatched_work_cannot_fabricate_a_quiet_receipt() {
        let transaction = LogTransaction::new(
            OperationId::fresh().expect("id"),
            TransportGeneration::new(1).expect("generation"),
            Duration::from_secs(1),
        )
        .expect("transaction");
        transaction.submit().expect("submit");
        transaction.mark_dispatched();
        transaction.finish();
        assert_eq!(
            transaction.cancel_and_wait().await,
            Err(LogProtocolError::CleanupTimeout)
        );
        transaction.finish_cleanup(Ok(()));
        assert_eq!(
            transaction.cancel_and_wait().await,
            Err(LogProtocolError::CleanupTimeout),
            "a later drop cannot overwrite failure evidence"
        );
    }
}
