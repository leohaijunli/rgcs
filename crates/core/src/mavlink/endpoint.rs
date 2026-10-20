//! Transport endpoints rendered to MAVLink address strings.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::MavlinkError;

/// A MAVLink transport endpoint (ADR-003: UDP or mavlink-router splitting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Endpoint {
    /// `udpin:<addr>` — UDP server, listens for incoming packets.
    UdpListener { addr: SocketAddr },
    /// `udpout:<addr>` — UDP client, sends to a fixed remote.
    UdpClient { addr: SocketAddr },
    /// `udpbcast:<addr>` — UDP broadcast sender.
    UdpBroadcast { addr: SocketAddr },
    /// `tcpin:<addr>` — TCP server, waits for one incoming connection.
    TcpServer { addr: SocketAddr },
    /// `tcpout:<addr>` — TCP client.
    TcpClient { addr: SocketAddr },
    /// `serial:<port>:<baudrate>` — serial port.
    Serial { port: PathBuf, baudrate: u32 },
}

/// Transport family of an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Udp,
    Tcp,
    Serial,
}

impl Endpoint {
    /// Render to a MAVLink address string accepted by `mavlink::connect`.
    pub fn to_address_string(&self) -> String {
        match self {
            Endpoint::UdpListener { addr } => format!("udpin:{addr}"),
            Endpoint::UdpClient { addr } => format!("udpout:{addr}"),
            Endpoint::UdpBroadcast { addr } => format!("udpbcast:{addr}"),
            Endpoint::TcpServer { addr } => format!("tcpin:{addr}"),
            Endpoint::TcpClient { addr } => format!("tcpout:{addr}"),
            Endpoint::Serial { port, baudrate } => {
                format!("serial:{}:{baudrate}", port.display())
            }
        }
    }

    /// Transport family.
    pub fn transport_kind(&self) -> TransportKind {
        match self {
            Endpoint::UdpListener { .. }
            | Endpoint::UdpClient { .. }
            | Endpoint::UdpBroadcast { .. } => TransportKind::Udp,
            Endpoint::TcpServer { .. } | Endpoint::TcpClient { .. } => TransportKind::Tcp,
            Endpoint::Serial { .. } => TransportKind::Serial,
        }
    }

    /// The remote address for connection-oriented endpoints, if any.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        match self {
            Endpoint::UdpClient { addr } | Endpoint::TcpClient { addr } => Some(*addr),
            _ => None,
        }
    }
}

impl From<&Endpoint> for String {
    fn from(value: &Endpoint) -> Self {
        value.to_address_string()
    }
}

/// Parse a MAVLink address string back into an [`Endpoint`].
///
/// Round-trips [`Endpoint::to_address_string`] for the formats the mavlink
/// crate supports.
impl TryFrom<&str> for Endpoint {
    type Error = MavlinkError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let (kind, rest) = value
            .split_once(':')
            .ok_or_else(|| MavlinkError::InvalidEndpoint(value.to_string()))?;
        match kind {
            "udpin" | "udpout" | "udpbcast" | "tcpin" | "tcpout" => {
                let addr = rest
                    .parse::<SocketAddr>()
                    .map_err(|_| MavlinkError::InvalidEndpoint(value.to_string()))?;
                match kind {
                    "udpin" => Ok(Endpoint::UdpListener { addr }),
                    "udpout" => Ok(Endpoint::UdpClient { addr }),
                    "udpbcast" => Ok(Endpoint::UdpBroadcast { addr }),
                    "tcpin" => Ok(Endpoint::TcpServer { addr }),
                    "tcpout" => Ok(Endpoint::TcpClient { addr }),
                    _ => unreachable!(),
                }
            }
            "serial" => {
                let (port, baud) = rest
                    .rsplit_once(':')
                    .ok_or_else(|| MavlinkError::InvalidEndpoint(value.to_string()))?;
                let baudrate = baud
                    .parse::<u32>()
                    .map_err(|_| MavlinkError::InvalidEndpoint(value.to_string()))?;
                Ok(Endpoint::Serial {
                    port: PathBuf::from(port),
                    baudrate,
                })
            }
            _ => Err(MavlinkError::InvalidEndpoint(value.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(e: Endpoint) {
        let s = e.to_address_string();
        let back: Endpoint = s.as_str().try_into().expect("parse");
        assert_eq!(e, back, "roundtrip failed for {s}");
    }

    #[test]
    fn udp_roundtrip() {
        roundtrip(Endpoint::UdpListener {
            addr: "0.0.0.0:14550".parse().unwrap(),
        });
        roundtrip(Endpoint::UdpClient {
            addr: "127.0.0.1:14550".parse().unwrap(),
        });
        roundtrip(Endpoint::UdpBroadcast {
            addr: "192.168.1.255:14550".parse().unwrap(),
        });
    }

    #[test]
    fn tcp_roundtrip() {
        roundtrip(Endpoint::TcpClient {
            addr: "127.0.0.1:5760".parse().unwrap(),
        });
        roundtrip(Endpoint::TcpServer {
            addr: "0.0.0.0:5760".parse().unwrap(),
        });
    }

    #[test]
    fn serial_roundtrip() {
        roundtrip(Endpoint::Serial {
            port: "/dev/ttyUSB0".into(),
            baudrate: 115_200,
        });
    }

    #[test]
    fn rejects_garbage() {
        let r: Result<Endpoint, _> = "banana".try_into();
        assert!(r.is_err());
    }
}
