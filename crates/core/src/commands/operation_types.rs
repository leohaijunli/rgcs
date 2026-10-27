//! Native command-operation payloads, generated from the reviewed Runtime contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{CommandError, CommandResult};
use crate::height::Height;
use crate::runtime::operations::{OperationError, OperationRef, RuntimeClientSession};
use crate::runtime::RequestId;
use crate::safety::{ChecklistItemId, ForceArmRequest, PlanState, PreflightRefusal};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandRequestContext {
    pub session: RuntimeClientSession,
    pub request_id: RequestId,
}

/// A terminal FC verdict, not proof that a reboot or other physical action finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandReply {
    pub operation: OperationRef,
    pub result: CommandResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CommandOperationError {
    #[error("{error}")]
    Operation { error: OperationError },
    #[error("{error}")]
    Protocol { error: CommandError },
    #[error("command outcome requires reconciliation: {reason}")]
    ReconciliationRequired {
        operation: Box<OperationRef>,
        reason: String,
    },
}

impl From<OperationError> for CommandOperationError {
    fn from(error: OperationError) -> Self {
        Self::Operation { error }
    }
}

impl From<CommandError> for CommandOperationError {
    fn from(error: CommandError) -> Self {
        Self::Protocol { error }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlightCommandIntent {
    Takeoff {
        plan_state: Option<PlanState>,
        acknowledged_warnings: Vec<ChecklistItemId>,
    },
    Land {
        ground_amsl: Option<Height>,
    },
    ReturnToLaunch,
    SetMode {
        main_mode: u8,
        sub_mode: u8,
    },
    Arm {
        force: Option<ForceArmRequest>,
        plan_state: Option<PlanState>,
        acknowledged_warnings: Vec<ChecklistItemId>,
    },
    Disarm,
    Goto {
        latitude_deg: f64,
        longitude_deg: f64,
    },
    SetHome {
        latitude_deg: f64,
        longitude_deg: f64,
        altitude: Option<Height>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FlightCommandRequest {
    pub context: CommandRequestContext,
    pub intent: FlightCommandIntent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FlightCommandKind {
    Takeoff,
    Land,
    ReturnToLaunch,
    SetMode,
    Arm,
    Disarm,
    Goto,
    SetHome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FlightCommandStep {
    Takeoff,
    Land,
    ReturnToLaunch,
    SetMode,
    Arm,
    Disarm,
    Goto,
    SetHome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FlightCommandStepResult {
    pub step: FlightCommandStep,
    pub result: CommandResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FlightCommandReply {
    pub operation: OperationRef,
    pub command: FlightCommandKind,
    pub steps: Vec<FlightCommandStepResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum FlightCommandError {
    #[error("{error}")]
    Command { error: CommandOperationError },
    #[error("{refusal:?}")]
    Preflight { refusal: PreflightRefusal },
}

impl From<CommandOperationError> for FlightCommandError {
    fn from(error: CommandOperationError) -> Self {
        Self::Command { error }
    }
}

impl From<OperationError> for FlightCommandError {
    fn from(error: OperationError) -> Self {
        CommandOperationError::from(error).into()
    }
}

impl From<CommandError> for FlightCommandError {
    fn from(error: CommandError) -> Self {
        CommandOperationError::from(error).into()
    }
}

impl From<PreflightRefusal> for FlightCommandError {
    fn from(refusal: PreflightRefusal) -> Self {
        Self::Preflight { refusal }
    }
}
