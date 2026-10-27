//! AF-02a: owner-bound, bounded operation coordination.
//!
//! The native adapter verifies the live target/client and runs existing protocol
//! services. This ledger owns resource claims/cancellation identity, not device
//! I/O. Cancellation or a lost caller never implies successful physical cleanup.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{ClientId, ModuleInstanceId, OperationId, RequestId, RuntimeError, TargetRef};

const DEFAULT_MAX_ACTIVE: usize = 32;
const DEFAULT_RESERVED_EMERGENCY: usize = 2;
const DEFAULT_MAX_RETAINED: usize = 128;
const DEFAULT_MAX_BUDGET_MS: u64 = 60_000;
const MAX_CONFIGURED_ACTIVE: usize = 256;
const MAX_CONFIGURED_RETAINED: usize = 4_096;
const MAX_CONFIGURED_BUDGET_MS: u64 = 86_400_000;
const RESOURCE_KINDS: usize = 8;
const MAX_OUTCOME_TEXT_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OperationOwner {
    pub client_id: ClientId,
    pub instance_id: ModuleInstanceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OperationRef {
    pub id: OperationId,
    pub owner: OperationOwner,
    pub target: TargetRef,
}

/// Native-issued module attachment; identifiers are checked against the actual
/// client/window registration before accepting an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuntimeClientSession {
    pub owner: OperationOwner,
    pub target: TargetRef,
    pub module_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OperationResource {
    ParameterTransfer,
    ParameterWrite,
    ActuatorOutput,
    MissionTransfer,
    Command,
    /// Safety command admission never waits behind another Runtime claim. The
    /// single command worker still serializes/preempts the physical protocol.
    EmergencyCommand,
    Shell,
    LogTransfer,
}

impl OperationResource {
    fn conflicts(self, other: Self) -> bool {
        use OperationResource::*;
        if matches!(self, EmergencyCommand) || matches!(other, EmergencyCommand) {
            return false;
        }
        self == other
            || matches!(
                (self, other),
                (ActuatorOutput, ParameterWrite | MissionTransfer | Command)
                    | (ParameterWrite | MissionTransfer | Command, ActuatorOutput)
                    | (Shell, LogTransfer)
                    | (LogTransfer, Shell)
                    | (ParameterTransfer, ParameterWrite)
                    | (ParameterWrite, ParameterTransfer)
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OperationRequest {
    pub request_id: RequestId,
    pub owner: OperationOwner,
    pub target: TargetRef,
    pub resources: Vec<OperationResource>,
    #[ts(type = "number")]
    pub budget_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Prepared,
    Running,
    CancelRequested,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    NotRequired,
    Confirmed,
    Pending,
    Failed,
    Uncertain,
}

impl CleanupOutcome {
    fn confirmed(self) -> bool {
        matches!(self, Self::NotRequired | Self::Confirmed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationOutcome {
    Succeeded,
    Cancelled,
    Failed { message: String },
    Uncertain { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OperationStopReason {
    OwnerRequest,
    DeadlineExceeded,
    TargetChanged,
    OwnerLost,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OperationStatus {
    pub operation: OperationRef,
    pub request_id: RequestId,
    pub resources: Vec<OperationResource>,
    pub phase: OperationPhase,
    pub cleanup: CleanupOutcome,
    pub outcome: Option<OperationOutcome>,
    pub stop_reason: Option<OperationStopReason>,
    #[ts(type = "number")]
    pub remaining_budget_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum OperationError {
    #[error("Runtime target error: {error}")]
    Runtime { error: RuntimeError },
    #[error("the client/module does not own this operation")]
    NotOwner,
    #[error("the operation is not retained")]
    NotFound,
    #[error("operation context does not match its original owner/target")]
    ContextMismatch,
    #[error("{resource:?} is owned by operation {holder:?}")]
    ResourceBusy {
        resource: OperationResource,
        holder: Box<OperationRef>,
    },
    #[error("the operation registry is at its configured capacity")]
    CapacityExceeded,
    #[error("this request is already recorded as {operation:?}; intent was not replayed")]
    DuplicateRequest { operation: Box<OperationRef> },
    #[error("operation cancellation was requested")]
    Cancelled,
    #[error("superseded by an admitted emergency command")]
    Superseded,
    #[error("the operation deadline expired")]
    DeadlineExceeded,
    #[error("the operation's bound target expired")]
    TargetChanged,
    #[error("the operation has already finished")]
    AlreadyFinished,
    #[error("invalid operation request: {message}")]
    InvalidRequest { message: String },
    #[error("the operation registry is unavailable")]
    RegistryUnavailable,
}

#[derive(Debug, Clone, Copy)]
pub struct CoordinatorConfig {
    pub max_active: usize,
    /// Slots unavailable to normal work so safety commands cannot be rejected
    /// merely because ordinary operation metadata reached its active budget.
    pub reserved_emergency: usize,
    pub max_retained: usize,
    pub max_budget_ms: u64,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            max_active: DEFAULT_MAX_ACTIVE,
            reserved_emergency: DEFAULT_RESERVED_EMERGENCY,
            max_retained: DEFAULT_MAX_RETAINED,
            max_budget_ms: DEFAULT_MAX_BUDGET_MS,
        }
    }
}

struct Record {
    status: OperationStatus,
    deadline: Instant,
    cancellation: OperationCancellation,
    holds_resources: bool,
    dispatched: bool,
}

impl Record {
    fn snapshot(&self) -> OperationStatus {
        let mut snapshot = self.status.clone();
        snapshot.remaining_budget_ms = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u64::MAX as u128) as u64;
        snapshot
    }

    fn stop(&mut self, reason: OperationStopReason) {
        if self.status.phase == OperationPhase::Finished {
            return;
        }
        if self.status.stop_reason.is_none() || reason == OperationStopReason::TargetChanged {
            self.status.stop_reason = Some(reason);
        }
        if self.dispatched {
            self.status.phase = OperationPhase::CancelRequested;
            self.status.cleanup = CleanupOutcome::Pending;
        } else {
            self.status.phase = OperationPhase::Finished;
            self.status.outcome = Some(OperationOutcome::Cancelled);
            self.status.cleanup = CleanupOutcome::NotRequired;
            self.holds_resources = false;
        }
        self.cancellation.cancel();
    }

    fn check(&mut self) -> Result<(), OperationError> {
        if Instant::now() >= self.deadline && self.status.phase != OperationPhase::Finished {
            self.stop(OperationStopReason::DeadlineExceeded);
        }
        match self.status.stop_reason {
            Some(OperationStopReason::DeadlineExceeded) => {
                return Err(OperationError::DeadlineExceeded)
            }
            Some(OperationStopReason::TargetChanged) => return Err(OperationError::TargetChanged),
            Some(OperationStopReason::Superseded) => return Err(OperationError::Superseded),
            Some(_) => return Err(OperationError::Cancelled),
            None => {}
        }
        if self.status.phase == OperationPhase::Finished {
            return Err(OperationError::AlreadyFinished);
        }
        Ok(())
    }
}

#[derive(Default)]
struct Ledger {
    records: BTreeMap<OperationId, Record>,
    order: VecDeque<OperationId>,
}

struct Inner {
    config: CoordinatorConfig,
    ledger: Mutex<Ledger>,
}

impl Inner {
    fn ledger(&self) -> Result<MutexGuard<'_, Ledger>, OperationError> {
        self.ledger
            .lock()
            .map_err(|_| OperationError::RegistryUnavailable)
    }
}

/// The registry has no task, transport or device lock. Its short mutex protects
/// only bounded metadata. Stop signals do not queue behind normal service work.
#[derive(Clone)]
pub struct OperationCoordinator {
    inner: Arc<Inner>,
}

impl Default for OperationCoordinator {
    fn default() -> Self {
        // Known, validated constants; default construction is inert and does
        // not allocate IDs, spawn workers, or acquire device ownership.
        Self {
            inner: Arc::new(Inner {
                config: CoordinatorConfig::default(),
                ledger: Mutex::new(Ledger::default()),
            }),
        }
    }
}

/// Read-only observation of the existing atomic/Notify cancellation primitive.
/// A slow observer cannot block Stop by retaining a watch-channel read lock.
#[derive(Clone, Default)]
pub struct OperationCancellation(crate::devices::SerialCancellation);

impl OperationCancellation {
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
    fn cancel(&self) {
        self.0.cancel();
    }
}

impl OperationCoordinator {
    pub fn new(config: CoordinatorConfig) -> Result<Self, OperationError> {
        if config.max_active == 0
            || config.max_active > MAX_CONFIGURED_ACTIVE
            || config.reserved_emergency > config.max_active
            || config.max_retained < config.max_active
            || config.max_retained > MAX_CONFIGURED_RETAINED
            || config.max_budget_ms == 0
            || config.max_budget_ms > MAX_CONFIGURED_BUDGET_MS
        {
            return Err(OperationError::InvalidRequest {
                message:
                    "valid capacity/budget, emergency reserve and retained >= active are required"
                        .into(),
            });
        }
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                ledger: Mutex::new(Ledger::default()),
            }),
        })
    }

    pub fn prepare(&self, mut request: OperationRequest) -> Result<OperationLease, OperationError> {
        request
            .target
            .validate()
            .map_err(|error| OperationError::Runtime { error })?;
        if request.resources.is_empty()
            || request.resources.len() > RESOURCE_KINDS
            || request.budget_ms == 0
        {
            return Err(OperationError::InvalidRequest {
                message: "operations need bounded nonempty resource claims and a positive budget"
                    .into(),
            });
        }
        let emergency = request
            .resources
            .contains(&OperationResource::EmergencyCommand);
        if emergency && request.resources.len() != 1 {
            return Err(OperationError::InvalidRequest {
                message: "EmergencyCommand is a standalone nonblocking resource marker".into(),
            });
        }
        request.resources.sort();
        if request.resources.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OperationError::InvalidRequest {
                message: "duplicate resource claims".into(),
            });
        }
        let budget = request.budget_ms.min(self.inner.config.max_budget_ms);
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(budget))
            .ok_or_else(|| OperationError::InvalidRequest {
                message: "deadline is not representable".into(),
            })?;
        // Randomness is I/O; do not acquire it under the metadata/cancel lock.
        let operation_id =
            OperationId::fresh().map_err(|error| OperationError::Runtime { error })?;
        let mut ledger = self.inner.ledger()?;
        for record in ledger.records.values() {
            if record.status.operation.owner == request.owner
                && record.status.request_id == request.request_id
            {
                return Err(OperationError::DuplicateRequest {
                    operation: Box::new(record.status.operation.clone()),
                });
            }
            if record.holds_resources
                && record.status.operation.target.vehicle_id == request.target.vehicle_id
            {
                for resource in &request.resources {
                    if record
                        .status
                        .resources
                        .iter()
                        .any(|held| resource.conflicts(*held))
                    {
                        return Err(OperationError::ResourceBusy {
                            resource: *resource,
                            holder: Box::new(record.status.operation.clone()),
                        });
                    }
                }
            }
        }
        let active = ledger
            .records
            .values()
            .filter(|record| record.holds_resources)
            .count();
        let limit = if emergency {
            self.inner.config.max_active
        } else {
            self.inner
                .config
                .max_active
                .saturating_sub(self.inner.config.reserved_emergency)
        };
        if active >= limit {
            return Err(OperationError::CapacityExceeded);
        }
        while ledger.records.len() >= self.inner.config.max_retained {
            let removable = ledger
                .order
                .iter()
                .position(|id| {
                    ledger.records.get(id).is_some_and(|record| {
                        !record.holds_resources && record.status.phase == OperationPhase::Finished
                    })
                })
                .ok_or(OperationError::CapacityExceeded)?;
            let id = ledger
                .order
                .remove(removable)
                .ok_or(OperationError::RegistryUnavailable)?;
            ledger.records.remove(&id);
        }
        let operation = OperationRef {
            id: operation_id,
            owner: request.owner,
            target: request.target,
        };
        let cancellation = OperationCancellation::default();
        let status = OperationStatus {
            operation: operation.clone(),
            request_id: request.request_id,
            resources: request.resources,
            phase: OperationPhase::Prepared,
            cleanup: CleanupOutcome::NotRequired,
            outcome: None,
            stop_reason: None,
            remaining_budget_ms: budget,
        };
        ledger.order.push_back(operation.id.clone());
        ledger.records.insert(
            operation.id.clone(),
            Record {
                status,
                deadline,
                cancellation: cancellation.clone(),
                holds_resources: true,
                dispatched: false,
            },
        );
        Ok(OperationLease {
            inner: self.inner.clone(),
            operation,
            cancellation,
            finished: false,
        })
    }

    pub fn cancel(
        &self,
        owner: &OperationOwner,
        operation: &OperationRef,
    ) -> Result<OperationStatus, OperationError> {
        if owner != &operation.owner {
            return Err(OperationError::NotOwner);
        }
        let mut ledger = self.inner.ledger()?;
        let record = record_mut(&mut ledger, operation)?;
        record.stop(OperationStopReason::OwnerRequest);
        Ok(record.snapshot())
    }

    pub fn status(
        &self,
        owner: &OperationOwner,
        operation: &OperationRef,
    ) -> Result<OperationStatus, OperationError> {
        if owner != &operation.owner {
            return Err(OperationError::NotOwner);
        }
        let mut ledger = self.inner.ledger()?;
        let record = record_mut(&mut ledger, operation)?;
        let _ = record.check();
        Ok(record.snapshot())
    }

    pub fn invalidate_target(
        &self,
        target: &TargetRef,
    ) -> Result<Vec<OperationStatus>, OperationError> {
        let mut ledger = self.inner.ledger()?;
        let mut affected = Vec::new();
        for record in ledger.records.values_mut() {
            if &record.status.operation.target == target
                && record.status.phase != OperationPhase::Finished
            {
                record.stop(OperationStopReason::TargetChanged);
                affected.push(record.snapshot());
            }
        }
        Ok(affected)
    }

    /// Native client expiry cancels only this captured client/module instance,
    /// never whichever operation happens to be current after a window reopen.
    pub fn cancel_owner(
        &self,
        owner: &OperationOwner,
    ) -> Result<Vec<OperationStatus>, OperationError> {
        let mut ledger = self.inner.ledger()?;
        let mut affected = Vec::new();
        for record in ledger.records.values_mut() {
            if &record.status.operation.owner == owner
                && record.status.phase != OperationPhase::Finished
            {
                record.stop(OperationStopReason::OwnerLost);
                affected.push(record.snapshot());
            }
        }
        Ok(affected)
    }

    /// Native arbitration for an admitted emergency. Cancels entire normal
    /// command operations, including gaps between their protocol steps. The
    /// original driver's cleanup receipt still controls resource release.
    pub fn supersede_commands(
        &self,
        emergency: &OperationRef,
    ) -> Result<Vec<OperationStatus>, OperationError> {
        let mut ledger = self.inner.ledger()?;
        let admitted = record_mut(&mut ledger, emergency)?;
        admitted.check()?;
        if admitted.status.phase != OperationPhase::Running
            || admitted.status.resources != [OperationResource::EmergencyCommand]
        {
            return Err(OperationError::InvalidRequest {
                message: "command supersession requires a running emergency operation".into(),
            });
        }
        let mut affected = Vec::new();
        for record in ledger.records.values_mut() {
            if record.status.operation.id != emergency.id
                && record.status.operation.target == emergency.target
                && record.status.phase != OperationPhase::Finished
                && record
                    .status
                    .resources
                    .contains(&OperationResource::Command)
            {
                record.stop(OperationStopReason::Superseded);
                affected.push(record.snapshot());
            }
        }
        Ok(affected)
    }

    /// Native authority calls this only after an actual owned cleanup/read-back
    /// receipt. It is not an unauthenticated client "clear busy" API.
    pub fn confirm_cleanup(
        &self,
        operation: &OperationRef,
        cleanup: CleanupOutcome,
    ) -> Result<OperationStatus, OperationError> {
        if !cleanup.confirmed() {
            return Err(OperationError::InvalidRequest {
                message: "cleanup confirmation requires explicit evidence".into(),
            });
        }
        let mut ledger = self.inner.ledger()?;
        let record = record_mut(&mut ledger, operation)?;
        if record.status.phase != OperationPhase::Finished {
            return Err(OperationError::InvalidRequest {
                message: "finish/reconcile the operation before confirming cleanup".into(),
            });
        }
        record.status.cleanup = cleanup;
        record.holds_resources = false;
        Ok(record.snapshot())
    }
}

fn record_mut<'a>(
    ledger: &'a mut Ledger,
    operation: &OperationRef,
) -> Result<&'a mut Record, OperationError> {
    let record = ledger
        .records
        .get_mut(&operation.id)
        .ok_or(OperationError::NotFound)?;
    if record.status.operation.owner != operation.owner {
        return Err(OperationError::NotOwner);
    }
    if &record.status.operation != operation {
        return Err(OperationError::ContextMismatch);
    }
    Ok(record)
}

/// One native worker's operation receipt. It cannot be cloned/reassigned to a
/// later operation. Drop signals cancellation and retains dispatched uncertainty.
pub struct OperationLease {
    inner: Arc<Inner>,
    operation: OperationRef,
    cancellation: OperationCancellation,
    finished: bool,
}

impl OperationLease {
    pub fn operation(&self) -> &OperationRef {
        &self.operation
    }
    pub fn cancellation(&self) -> OperationCancellation {
        self.cancellation.clone()
    }

    pub fn mark_running(&self) -> Result<(), OperationError> {
        let mut ledger = self.inner.ledger()?;
        let record = record_mut(&mut ledger, &self.operation)?;
        record.check()?;
        if record.status.phase != OperationPhase::Prepared {
            return Err(OperationError::InvalidRequest {
                message: "operation already dispatched".into(),
            });
        }
        record.dispatched = true;
        record.status.phase = OperationPhase::Running;
        Ok(())
    }

    pub fn check(&self) -> Result<(), OperationError> {
        let mut ledger = self.inner.ledger()?;
        record_mut(&mut ledger, &self.operation)?.check()
    }

    pub fn finish(
        mut self,
        outcome: OperationOutcome,
        cleanup: CleanupOutcome,
    ) -> Result<OperationStatus, OperationError> {
        let text_len = match &outcome {
            OperationOutcome::Failed { message } => message.len(),
            OperationOutcome::Uncertain { reason } => reason.len(),
            _ => 0,
        };
        if text_len > MAX_OUTCOME_TEXT_BYTES
            || (outcome == OperationOutcome::Succeeded && !cleanup.confirmed())
        {
            return Err(OperationError::InvalidRequest {
                message: "outcome evidence is oversized or successful cleanup is unconfirmed"
                    .into(),
            });
        }
        let mut ledger = self.inner.ledger()?;
        let record = record_mut(&mut ledger, &self.operation)?;
        let _ = record.check();
        if record.status.phase == OperationPhase::Finished {
            return Err(OperationError::AlreadyFinished);
        }
        if outcome == OperationOutcome::Succeeded {
            record.check()?;
            if !record.dispatched {
                return Err(OperationError::InvalidRequest {
                    message: "success requires a recorded running/dispatch step".into(),
                });
            }
        }
        record.status.phase = OperationPhase::Finished;
        record.status.outcome = Some(outcome);
        record.status.cleanup = cleanup;
        record.holds_resources = !cleanup.confirmed();
        record.cancellation.cancel();
        let snapshot = record.snapshot();
        self.finished = true;
        Ok(snapshot)
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.cancellation.cancel();
        let Ok(mut ledger) = self.inner.ledger() else {
            return;
        };
        let Ok(record) = record_mut(&mut ledger, &self.operation) else {
            return;
        };
        if record.status.phase == OperationPhase::Finished {
            return;
        }
        record
            .status
            .stop_reason
            .get_or_insert(OperationStopReason::OwnerLost);
        record.status.phase = OperationPhase::Finished;
        if record.dispatched {
            record.status.outcome = Some(OperationOutcome::Uncertain {
                reason: "operation worker/owner disappeared after dispatch".into(),
            });
            record.status.cleanup = CleanupOutcome::Uncertain;
        } else {
            record.status.outcome = Some(OperationOutcome::Cancelled);
            record.status.cleanup = CleanupOutcome::NotRequired;
            record.holds_resources = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{LinkId, MavlinkTarget, TransportGeneration, VehicleId, VehicleSessionId};

    fn owner(name: &str) -> OperationOwner {
        OperationOwner {
            client_id: ClientId::new(name).expect("client"),
            instance_id: ModuleInstanceId::new(name).expect("instance"),
        }
    }
    fn request(name: &str, resource: OperationResource) -> OperationRequest {
        OperationRequest {
            request_id: RequestId::fresh().expect("request"),
            owner: owner(name),
            target: TargetRef {
                vehicle_id: VehicleId::new("aircraft-a").expect("vehicle"),
                session_id: VehicleSessionId::new("session-a").expect("session"),
                link_id: LinkId::new(1).expect("link"),
                generation: TransportGeneration::new(1).expect("generation"),
                mavlink_target: MavlinkTarget::new(42, 1),
            },
            resources: vec![resource],
            budget_ms: 5000,
        }
    }
    fn coordinator() -> OperationCoordinator {
        OperationCoordinator::new(CoordinatorConfig::default()).expect("coordinator")
    }

    #[test]
    fn another_client_or_instance_cannot_cancel_owned_work() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("b", OperationResource::ParameterWrite))
            .expect("prepare");
        assert_eq!(
            coordinator.cancel(&owner("a"), lease.operation()),
            Err(OperationError::NotOwner)
        );
        let mut wrong_instance = owner("b");
        wrong_instance.instance_id = ModuleInstanceId::new("another-instance").expect("instance");
        assert_eq!(
            coordinator.cancel(&wrong_instance, lease.operation()),
            Err(OperationError::NotOwner)
        );
        assert!(lease.check().is_ok());
    }

    #[test]
    fn cancellation_before_dispatch_releases_only_its_claim() {
        let coordinator = coordinator();
        let first = coordinator
            .prepare(request("a", OperationResource::ParameterWrite))
            .expect("prepare");
        let status = coordinator
            .cancel(&owner("a"), first.operation())
            .expect("cancel");
        assert_eq!(status.outcome, Some(OperationOutcome::Cancelled));
        assert_eq!(first.mark_running(), Err(OperationError::Cancelled));
        let second = coordinator
            .prepare(request("b", OperationResource::ParameterWrite))
            .expect("new owner");
        coordinator
            .cancel(&owner("a"), first.operation())
            .expect("idle repeat");
        assert!(second.check().is_ok());
    }

    #[test]
    fn cancellation_after_dispatch_does_not_claim_cleanup_success() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::ActuatorOutput))
            .expect("prepare");
        lease.mark_running().expect("running");
        let token = lease.cancellation();
        let status = coordinator
            .cancel(&owner("a"), lease.operation())
            .expect("cancel");
        assert!(token.is_cancelled());
        assert_eq!(status.phase, OperationPhase::CancelRequested);
        assert_eq!(status.cleanup, CleanupOutcome::Pending);
        assert!(matches!(
            coordinator.prepare(request("b", OperationResource::ActuatorOutput)),
            Err(OperationError::ResourceBusy { .. })
        ));
        lease
            .finish(OperationOutcome::Cancelled, CleanupOutcome::Confirmed)
            .expect("release receipt");
        assert!(coordinator
            .prepare(request("b", OperationResource::ActuatorOutput))
            .is_ok());
    }

    #[test]
    fn conflicted_resources_are_per_vehicle_and_reads_need_no_claim() {
        let coordinator = coordinator();
        let _motor = coordinator
            .prepare(request("a", OperationResource::ActuatorOutput))
            .expect("motor");
        assert!(matches!(
            coordinator.prepare(request("b", OperationResource::ParameterWrite)),
            Err(OperationError::ResourceBusy { .. })
        ));
        let _capture = coordinator
            .prepare(request("b", OperationResource::Shell))
            .expect("independent capture");
        let mut other = request("c", OperationResource::ActuatorOutput);
        other.target.vehicle_id = VehicleId::new("aircraft-b").expect("vehicle");
        assert!(coordinator.prepare(other).is_ok());
    }

    #[test]
    fn emergency_commands_never_wait_behind_runtime_resource_claims() {
        let coordinator = coordinator();
        let _normal = coordinator
            .prepare(request("normal", OperationResource::Command))
            .expect("normal command");
        assert!(matches!(
            coordinator.prepare(request("motor", OperationResource::ActuatorOutput)),
            Err(OperationError::ResourceBusy { .. })
        ));
        let _first = coordinator
            .prepare(request("rtl", OperationResource::EmergencyCommand))
            .expect("RTL bypasses normal claim");
        let _second = coordinator
            .prepare(request("disarm", OperationResource::EmergencyCommand))
            .expect("the command worker, not metadata, arbitrates emergencies");
    }

    #[test]
    fn active_budget_reserves_capacity_for_an_emergency() {
        let coordinator = OperationCoordinator::new(CoordinatorConfig {
            max_active: 3,
            reserved_emergency: 1,
            max_retained: 4,
            max_budget_ms: 1000,
        })
        .expect("config");
        let mut first = request("first", OperationResource::Shell);
        first.target.vehicle_id = VehicleId::new("aircraft-a").expect("vehicle");
        let mut second = request("second", OperationResource::Shell);
        second.target.vehicle_id = VehicleId::new("aircraft-b").expect("vehicle");
        let mut third = request("third", OperationResource::Shell);
        third.target.vehicle_id = VehicleId::new("aircraft-c").expect("vehicle");
        let mut emergency = request("rtl", OperationResource::EmergencyCommand);
        emergency.target.vehicle_id = VehicleId::new("aircraft-a").expect("vehicle");
        let _first = coordinator.prepare(first).expect("first normal");
        let _second = coordinator.prepare(second).expect("second normal");
        assert!(matches!(
            coordinator.prepare(third),
            Err(OperationError::CapacityExceeded)
        ));
        assert!(coordinator.prepare(emergency).is_ok());
    }

    #[test]
    fn dropping_dispatched_work_retains_uncertainty_and_claims() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::ParameterWrite))
            .expect("prepare");
        let operation = lease.operation().clone();
        lease.mark_running().expect("running");
        drop(lease);
        let status = coordinator
            .status(&owner("a"), &operation)
            .expect("journal");
        assert!(matches!(
            status.outcome,
            Some(OperationOutcome::Uncertain { .. })
        ));
        assert_eq!(status.cleanup, CleanupOutcome::Uncertain);
        assert!(matches!(
            coordinator.prepare(request("b", OperationResource::ParameterWrite)),
            Err(OperationError::ResourceBusy { .. })
        ));
        coordinator
            .confirm_cleanup(&operation, CleanupOutcome::Confirmed)
            .expect("reconcile");
        assert!(coordinator
            .prepare(request("b", OperationResource::ParameterWrite))
            .is_ok());
    }

    #[test]
    fn dropping_prepared_work_cancels_without_claiming_a_dispatched_effect() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::Shell))
            .expect("prepare");
        let operation = lease.operation().clone();
        drop(lease);
        let status = coordinator.status(&owner("a"), &operation).expect("status");
        assert_eq!(status.outcome, Some(OperationOutcome::Cancelled));
        assert_eq!(status.cleanup, CleanupOutcome::NotRequired);
        assert!(coordinator
            .prepare(request("b", OperationResource::Shell))
            .is_ok());
    }

    #[test]
    fn a_duplicate_request_is_reported_without_replaying_or_refreshing_budget() {
        let coordinator = coordinator();
        let original = request("a", OperationResource::ParameterWrite);
        let lease = coordinator.prepare(original.clone()).expect("prepare");
        let error = coordinator.prepare(original).err().expect("duplicate");
        assert_eq!(
            error,
            OperationError::DuplicateRequest {
                operation: Box::new(lease.operation().clone())
            }
        );
        assert_eq!(coordinator.inner.ledger().expect("ledger").records.len(), 1);
    }

    #[test]
    fn an_old_target_result_cannot_complete_replacement_work() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::ParameterWrite))
            .expect("prepare");
        let original = lease.operation().clone();
        lease.mark_running().expect("running");
        coordinator
            .invalidate_target(&original.target)
            .expect("retire target");
        assert_eq!(lease.check(), Err(OperationError::TargetChanged));
        assert_eq!(
            lease.finish(OperationOutcome::Succeeded, CleanupOutcome::NotRequired),
            Err(OperationError::TargetChanged)
        );
        let status = coordinator
            .status(&owner("a"), &original)
            .expect("evidence");
        assert!(matches!(
            status.outcome,
            Some(OperationOutcome::Uncertain { .. })
        ));
        let mut changed = original.clone();
        changed.target.generation = TransportGeneration::new(2).expect("generation");
        assert_eq!(
            coordinator.cancel(&owner("a"), &changed),
            Err(OperationError::ContextMismatch)
        );
    }

    #[test]
    fn cancelled_late_success_is_not_a_terminal_success() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::ParameterWrite))
            .expect("prepare");
        let operation = lease.operation().clone();
        lease.mark_running().expect("running");
        coordinator.cancel(&owner("a"), &operation).expect("cancel");
        assert_eq!(
            lease.finish(OperationOutcome::Succeeded, CleanupOutcome::NotRequired),
            Err(OperationError::Cancelled)
        );
        assert!(matches!(
            coordinator
                .status(&owner("a"), &operation)
                .expect("retained")
                .outcome,
            Some(OperationOutcome::Uncertain { .. })
        ));
    }

    #[test]
    fn registry_capacity_is_bounded_and_pinned_uncertainty_is_not_evicted() {
        let coordinator = OperationCoordinator::new(CoordinatorConfig {
            max_active: 1,
            reserved_emergency: 0,
            max_retained: 1,
            max_budget_ms: 1000,
        })
        .expect("config");
        let lease = coordinator
            .prepare(request("a", OperationResource::Shell))
            .expect("prepare");
        lease.mark_running().expect("running");
        drop(lease);
        let mut other = request("b", OperationResource::Command);
        other.target.vehicle_id = VehicleId::new("other").expect("vehicle");
        assert!(matches!(
            coordinator.prepare(other),
            Err(OperationError::CapacityExceeded)
        ));
        assert_eq!(coordinator.inner.ledger().expect("ledger").records.len(), 1);
    }

    #[test]
    fn completed_unpinned_history_is_evicted_within_budget() {
        let coordinator = OperationCoordinator::new(CoordinatorConfig {
            max_active: 1,
            reserved_emergency: 0,
            max_retained: 2,
            max_budget_ms: 1000,
        })
        .expect("config");
        for _ in 0..5 {
            let lease = coordinator
                .prepare(request("a", OperationResource::Shell))
                .expect("prepare");
            lease.mark_running().expect("running");
            lease
                .finish(OperationOutcome::Succeeded, CleanupOutcome::NotRequired)
                .expect("finish");
        }
        let ledger = coordinator.inner.ledger().expect("ledger");
        assert_eq!(ledger.records.len(), 2);
        assert_eq!(ledger.order.len(), 2);
    }

    #[test]
    fn expiry_preserves_dispatched_uncertainty_and_does_not_release_resources() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::Command))
            .expect("prepare");
        lease.mark_running().expect("running");
        let operation = lease.operation().clone();
        coordinator
            .inner
            .ledger()
            .expect("ledger")
            .records
            .get_mut(&operation.id)
            .expect("record")
            .deadline = Instant::now();
        assert_eq!(lease.check(), Err(OperationError::DeadlineExceeded));
        let status = coordinator.status(&owner("a"), &operation).expect("status");
        assert_eq!(status.cleanup, CleanupOutcome::Pending);
        assert_eq!(status.remaining_budget_ms, 0);
        assert!(matches!(
            coordinator.prepare(request("b", OperationResource::Command)),
            Err(OperationError::ResourceBusy { .. })
        ));
    }

    #[test]
    fn request_and_outcome_budgets_are_validated() {
        assert!(OperationCoordinator::new(CoordinatorConfig {
            max_active: 2,
            reserved_emergency: 0,
            max_retained: 1,
            max_budget_ms: 1
        })
        .is_err());
        let coordinator = coordinator();
        let mut invalid = request("a", OperationResource::Shell);
        invalid.resources.push(OperationResource::Shell);
        assert!(matches!(
            coordinator.prepare(invalid),
            Err(OperationError::InvalidRequest { .. })
        ));
        let lease = coordinator
            .prepare(request("a", OperationResource::Shell))
            .expect("prepare");
        lease.mark_running().expect("running");
        assert!(matches!(
            lease.finish(OperationOutcome::Succeeded, CleanupOutcome::Pending),
            Err(OperationError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn success_cannot_skip_the_recorded_dispatch_boundary() {
        let coordinator = coordinator();
        let lease = coordinator
            .prepare(request("a", OperationResource::ParameterWrite))
            .expect("prepare");
        let operation = lease.operation().clone();
        assert!(matches!(
            lease.finish(OperationOutcome::Succeeded, CleanupOutcome::NotRequired),
            Err(OperationError::InvalidRequest { .. })
        ));
        let status = coordinator.status(&owner("a"), &operation).expect("status");
        assert_eq!(status.outcome, Some(OperationOutcome::Cancelled));
        assert_eq!(status.cleanup, CleanupOutcome::NotRequired);
    }

    #[test]
    fn captured_owner_expiry_does_not_cancel_another_module_instance() {
        let coordinator = OperationCoordinator::default();
        let a = coordinator
            .prepare(request("a", OperationResource::ParameterWrite))
            .expect("a");
        let b = coordinator
            .prepare(request("b", OperationResource::Shell))
            .expect("b");
        a.mark_running().expect("running a");
        b.mark_running().expect("running b");
        let affected = coordinator.cancel_owner(&owner("a")).expect("expire a");
        assert_eq!(affected.len(), 1);
        assert_eq!(a.check(), Err(OperationError::Cancelled));
        assert!(b.check().is_ok());
        assert!(!b.cancellation().is_cancelled());
    }
}
