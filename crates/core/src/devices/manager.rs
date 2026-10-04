//! Device enumeration and hotplug watching (Phase 0, task 3).
//!
//! Enumeration uses `serialport::available_ports` and maps the result into
//! [`SerialDeviceInfo`]. The watcher polls at a fixed interval and diffs the
//! port set, which is platform-neutral; a udev-event-driven watcher can
//! replace it on Linux later without changing the event surface.

use std::time::Duration;

use tokio::sync::broadcast;

use super::database::DeviceDatabase;
use super::device::{DeviceId, PortTransport, SerialDeviceInfo};
use super::error::DeviceError;

/// Default polling interval for the hotplug watcher.
pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Hotplug events emitted by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Added(SerialDeviceInfo),
    Removed(SerialDeviceInfo),
}

/// Event bus capacity for hotplug events.
pub const EVENT_CAPACITY: usize = 128;

/// Manages serial device enumeration and hotplug watching.
#[derive(Debug, Clone)]
pub struct DeviceManager {
    database: DeviceDatabase,
    watch_interval: Duration,
}

/// Handle returned by [`DeviceManager::spawn_watcher`].
#[derive(Clone)]
pub struct DeviceManagerHandle {
    events: broadcast::Sender<DeviceEvent>,
}

impl DeviceManager {
    /// New manager over a device database.
    pub fn new(database: DeviceDatabase) -> Self {
        Self {
            database,
            watch_interval: DEFAULT_WATCH_INTERVAL,
        }
    }

    /// Set the watcher polling interval.
    pub fn with_watch_interval(mut self, interval: Duration) -> Self {
        self.watch_interval = interval;
        self
    }

    /// Database used for role suggestions.
    pub fn database(&self) -> &DeviceDatabase {
        &self.database
    }

    /// Enumerate currently attached serial ports with identity metadata.
    pub fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError> {
        let ports = serialport::available_ports()?;
        Ok(ports
            .into_iter()
            .map(|p| {
                let (transport, device_id, serial_number, manufacturer, product) = match p.port_type
                {
                    serialport::SerialPortType::UsbPort(info) => (
                        PortTransport::Usb,
                        Some(DeviceId {
                            vendor_id: info.vid,
                            product_id: info.pid,
                        }),
                        info.serial_number,
                        info.manufacturer,
                        info.product,
                    ),
                    serialport::SerialPortType::PciPort => {
                        (PortTransport::Pci, None, None, None, None)
                    }
                    serialport::SerialPortType::BluetoothPort => {
                        (PortTransport::Bluetooth, None, None, None, None)
                    }
                    serialport::SerialPortType::Unknown => {
                        (PortTransport::Unknown, None, None, None, None)
                    }
                };
                SerialDeviceInfo {
                    port_name: p.port_name,
                    transport,
                    device_id,
                    serial_number,
                    manufacturer,
                    product,
                }
            })
            .collect())
    }

    /// Spawn the hotplug watcher task.
    ///
    /// Returns a handle; subscribe to receive [`DeviceEvent`]s. The caller
    /// reconciles the initial state by calling [`DeviceManager::enumerate`]
    /// before subscribing.
    pub fn spawn_watcher(self) -> Result<DeviceManagerHandle, DeviceError> {
        let (events_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let interval = self.watch_interval;
        let mut previous = self.enumerate()?;

        let tx = events_tx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                let current = match self.enumerate() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                for dev in current.diff(&previous, |a, b| a == b) {
                    let _ = tx.send(DeviceEvent::Added(dev));
                }
                for dev in previous.diff(&current, |a, b| a == b) {
                    let _ = tx.send(DeviceEvent::Removed(dev));
                }
                previous = current;
            }
        });

        Ok(DeviceManagerHandle { events: events_tx })
    }
}

impl DeviceManagerHandle {
    /// Subscribe to hotplug events.
    pub fn subscribe(&self) -> broadcast::Receiver<DeviceEvent> {
        self.events.subscribe()
    }
}

/// Set-difference helper that preserves equality semantics.
trait Diff<T> {
    fn diff<F>(&self, other: &[T], eq: F) -> Vec<T>
    where
        F: Fn(&T, &T) -> bool;
}

impl<T: Clone> Diff<T> for Vec<T> {
    fn diff<F>(&self, other: &[T], eq: F) -> Vec<T>
    where
        F: Fn(&T, &T) -> bool,
    {
        self.iter()
            .filter(|a| !other.iter().any(|b| eq(a, b)))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_added_and_removed() {
        let before = vec![1, 2, 3];
        let after = vec![2, 3, 4];
        assert_eq!(after.diff(&before, |a, b| a == b), vec![4]);
        assert_eq!(before.diff(&after, |a, b| a == b), vec![1]);
    }
}
