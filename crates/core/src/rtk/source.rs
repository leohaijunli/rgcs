//! RTCM base-station sources (comprehensive plan R1): anything that can feed
//! raw RTCM bytes into the injection service. The F9P serial port is the
//! primary source (operator decision 2026-10-11); a test double covers the
//! service tests.

use std::time::Duration;

/// Reasons a source cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error(transparent)]
    Discovery(#[from] crate::devices::SerialDiscoveryError),
    #[error("receiver I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("serial port {port}: {source}")]
    Serial {
        port: String,
        #[source]
        source: serialport::Error,
    },
    /// Not a port an operator would plug a base station into (the RTK UI
    /// lists the same operator-pluggable set the devices page shows:
    /// `/dev/ttyACM*`, `/dev/ttyUSB*`, Windows `COM*`).
    #[error("{port} is not an operator-pluggable serial port")]
    NotAnOperatorPort { port: String },
    /// The source was stopped while reading (normal shutdown).
    #[error("source stopped")]
    Stopped,
}

/// The configuration for a serial base station.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SerialSourceConfig {
    /// OS port name, e.g. `/dev/ttyUSB0` or `COM3`.
    pub port_name: String,
    /// Baud rate (F9P: 38400 default, 115200 for high MSM rates).
    pub baud_rate: u32,
}

/// A byte stream feeding RTCM into the injection service. Implemented by
/// the F9P serial port ([`SerialSource`]) and by the test double.
pub trait RtcmSource: Send {
    /// Read one chunk of bytes. Returns `Ok(None)` when no data is available
    /// right now; implementations should return within ~100 ms so the
    /// caller's stop flag gets a chance to be observed.
    fn read(&mut self) -> Result<Option<Vec<u8>>, SourceError>;
}

impl std::fmt::Debug for SerialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialSource")
            .field("port_name", &self.config.port_name)
            .field("baud_rate", &self.config.baud_rate)
            .finish()
    }
}

/// An open serial port wrapped in the [`RtcmSource`] shape. Reads are
/// non-blocking-ish: a 100 ms read timeout keeps the reader loop responsive
/// to stop requests.
pub struct SerialSource {
    port: Box<dyn serialport::SerialPort>,
    // (not Debug: the port handle is opaque)
    #[allow(dead_code)] // kept for the stats source label
    config: SerialSourceConfig,
    // Fields drop in declaration order: the OS port closes before its lease.
    _lease: Option<crate::devices::SerialLease>,
}

impl SerialSource {
    pub fn open(config: SerialSourceConfig) -> Result<Self, SourceError> {
        // Same filter the devices page enumerates with (COM* on Windows,
        // ttyACM*/ttyUSB* on Linux) — the RTK source never touches other
        // ports, so a typo cannot reach e.g. an internal ttyS*.
        if !crate::devices::manager::is_operator_port(&config.port_name) {
            return Err(SourceError::NotAnOperatorPort {
                port: config.port_name.clone(),
            });
        }
        let port = serialport::new(&config.port_name, config.baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|source| SourceError::Serial {
                port: config.port_name.clone(),
                source,
            })?;
        Ok(Self {
            port,
            config,
            _lease: None,
        })
    }

    pub fn open_leased(
        config: SerialSourceConfig,
        lease: crate::devices::SerialLease,
    ) -> Result<Self, SourceError> {
        if !lease.matches_path(std::path::Path::new(&config.port_name)) {
            return Err(crate::devices::SerialDiscoveryError::DeviceChanged {
                instance: lease.port().instance_id,
            }
            .into());
        }
        let mut source = Self::open(config)?;
        lease.set_phase(crate::devices::LeasePhase::Active)?;
        source._lease = Some(lease);
        Ok(source)
    }

    pub(crate) async fn validate_attachment(&self) -> Result<(), SourceError> {
        if let Some(lease) = &self._lease {
            lease.revalidate().await?;
        }
        Ok(())
    }
}

impl std::io::Read for SerialSource {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.port.read(bytes)
    }
}

impl std::io::Write for SerialSource {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.port.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.port.flush()
    }
}

impl super::ubx::ReceiverIo for SerialSource {
    fn set_baud_rate(&mut self, baud: u32) -> Result<(), SourceError> {
        self.port
            .set_baud_rate(baud)
            .map_err(|source| SourceError::Serial {
                port: self.config.port_name.clone(),
                source,
            })
    }
}

impl RtcmSource for SerialSource {
    fn read(&mut self) -> Result<Option<Vec<u8>>, SourceError> {
        // A serialport::SerialPort exposes its concrete read API via the
        // `serialport::SerialPort` trait's `read` through std::io::Read.
        use std::io::Read;
        let mut buf = [0u8; 512];
        match self.port.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                Ok(None)
            }
            Err(e) => Err(SourceError::Serial {
                port: self.config.port_name.clone(),
                source: serialport::Error::new(serialport::ErrorKind::Unknown, e.to_string()),
            }),
        }
    }
}

/// Test double: replays scripted byte chunks, then parks (returning `None`)
/// until stopped or exhausted.
pub struct FakeSource {
    chunks: std::collections::VecDeque<Vec<u8>>,
    chunk_delay: Duration,
}

impl FakeSource {
    pub fn new(chunks: Vec<Vec<u8>>, chunk_delay: Duration) -> Self {
        Self {
            chunks: chunks.into(),
            chunk_delay,
        }
    }
}

impl RtcmSource for FakeSource {
    fn read(&mut self) -> Result<Option<Vec<u8>>, SourceError> {
        std::thread::sleep(self.chunk_delay.min(Duration::from_millis(5)));
        Ok(self.chunks.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_operator_ports() {
        for port in ["/dev/ttyS0", "/dev/tty", "COM", "COMX", "COMA1", ""] {
            let err = SerialSource::open(SerialSourceConfig {
                port_name: port.into(),
                baud_rate: 38400,
            })
            .expect_err("must reject");
            assert!(
                matches!(err, SourceError::NotAnOperatorPort { .. }),
                "{port}"
            );
        }
        // Operator-named ports pass the filter and fail on the (absent)
        // hardware with a Serial error instead.
        for port in ["/dev/ttyUSB99999999", "/dev/ttyACM99999999", "COM99999999"] {
            let err = SerialSource::open(SerialSourceConfig {
                port_name: port.into(),
                baud_rate: 38400,
            })
            .expect_err("no hardware in CI");
            assert!(matches!(err, SourceError::Serial { .. }), "{port}");
        }
    }
}
