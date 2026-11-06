//! Throwaway diagnostic: byte-level activity scan across baud rates.
//! Run: cargo run -p maggcs-core --example serial_listen [-- /dev/ttyUSB0 57600 115200 ...]
//!
//! A live MAVLink v2 link shows a high byte count and a high density of 0xFD
//! start bytes; a wrong baud shows garbage or silence. Parsing is deliberately
//! skipped so a garbled stream can never stall the scan.

use std::io::Read;
use std::time::{Duration, Instant};

const PORT: &str = "/dev/ttyUSB0";
const DEFAULT_BAUDS: [u32; 6] = [115_200, 57_600, 38_400, 9_600, 230_400, 460_800];
const SCAN: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_millis(150);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (port, bauds): (String, Vec<u32>) = match args.split_first() {
        Some((p, rest)) if !rest.is_empty() => (
            p.clone(),
            rest.iter().filter_map(|b| b.parse().ok()).collect(),
        ),
        Some((p, _)) => (p.clone(), DEFAULT_BAUDS.to_vec()),
        None => (PORT.to_owned(), DEFAULT_BAUDS.to_vec()),
    };
    for baud in bauds {
        let mut handle = match serialport::new(&port, baud).timeout(READ_TIMEOUT).open() {
            Ok(h) => h,
            Err(e) => {
                println!("{baud:>7}: open failed: {e}");
                continue;
            }
        };
        let deadline = Instant::now() + SCAN;
        let (mut bytes, mut magic) = (0u64, 0u64);
        let mut buf = [0u8; 512];
        while Instant::now() < deadline {
            match handle.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    bytes += n as u64;
                    magic += buf[..n].iter().filter(|&&b| b == 0xFD).count() as u64;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => {
                    println!("{baud:>7}: read error: {e}");
                    break;
                }
            }
        }
        let density = if bytes > 0 {
            (magic as f64 / bytes as f64 * 1000.0).round() / 10.0
        } else {
            0.0
        };
        println!("{baud:>7}: bytes={bytes} 0xFD={magic} ({density}% of bytes)");
        if bytes > 50 && magic * 128 > bytes {
            println!("         -> {baud} baud looks like a live MAVLink link");
        }
    }
}
