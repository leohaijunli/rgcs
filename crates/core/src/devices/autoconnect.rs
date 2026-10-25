//! Shared serial connection settings primitives and cancellation.

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::SerialDiscoveryError;

pub const SERIAL_BAUD_RATES: [u32; 7] = [9_600, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "kind", content = "rate", rename_all = "snake_case")]
pub enum BaudSelection {
    #[default]
    Auto,
    Fixed(u32),
}

impl BaudSelection {
    pub fn validate(self) -> Result<(), SerialDiscoveryError> {
        if let Self::Fixed(baud) = self {
            if !SERIAL_BAUD_RATES.contains(&baud) {
                return Err(SerialDiscoveryError::InvalidBaud { baud });
            }
        }
        Ok(())
    }

    pub fn rate_or_auto(self) -> u32 {
        match self {
            Self::Auto => 0,
            Self::Fixed(baud) => baud,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SerialCancellation(watch::Sender<bool>);

impl Default for SerialCancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialCancellation {
    pub fn new() -> Self {
        Self(watch::channel(false).0)
    }
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub fn check(&self) -> Result<(), SerialDiscoveryError> {
        if self.is_cancelled() {
            Err(SerialDiscoveryError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub async fn cancelled(&self) {
        let _ = self.0.subscribe().wait_for(|cancelled| *cancelled).await;
    }
}
