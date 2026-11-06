//! Throwaway diagnostic: enumerate serial ports exactly like MagGCS does.
//! Run: cargo run -p maggcs-core --example serial_probe
use maggcs_core::devices::{DeviceDatabase, DeviceRole};

fn main() {
    println!("serialport enumeration:");
    let ports = serialport::available_ports().expect("available_ports");
    if ports.is_empty() {
        println!("  (none returned by the OS)");
    }
    let db = DeviceDatabase::builtin();
    for p in &ports {
        let detail = match &p.port_type {
            serialport::SerialPortType::UsbPort(info) => format!(
                "USB vid={:04x} pid={:04x} serial={:?} mfg={:?} product={:?}",
                info.vid, info.pid, info.serial_number, info.manufacturer, info.product
            ),
            serialport::SerialPortType::PciPort => "PCI".into(),
            serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
            serialport::SerialPortType::Unknown => "Unknown".into(),
        };
        let operator = maggcs_core::devices::manager::is_operator_port(&p.port_name);
        let device_id = match &p.port_type {
            serialport::SerialPortType::UsbPort(info) => maggcs_core::devices::DeviceId::from_hex(
                &format!("{:04x}", info.vid),
                &format!("{:04x}", info.pid),
            ),
            _ => None,
        };
        let role = device_id
            .as_ref()
            .and_then(|id| db.lookup(id))
            .map(|e| e.role)
            .unwrap_or(DeviceRole::Other);
        println!(
            "  {} -> operator_port={operator} role={role:?}\n    {detail}",
            p.port_name
        );
        match serialport::new(&p.port_name, 57_600)
            .timeout(std::time::Duration::from_millis(50))
            .open()
        {
            Ok(_port) => println!("    open@57600: OK"),
            Err(e) => println!("    open@57600: ERR {e}"),
        }
    }
}
