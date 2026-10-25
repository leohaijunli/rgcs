use super::*;
use crate::devices::{
    InventoryRevision, LeasePhase, PortInstanceId, PortTransport, SerialLeaseId, SerialLeaseStatus,
    SerialPortSnapshot,
};

fn port(
    id: u64,
    path: &str,
    serial: Option<&str>,
    role: DeviceRole,
    device_id: DeviceId,
) -> SerialPortSnapshot {
    let device = SerialDeviceInfo {
        port_name: path.into(),
        transport: PortTransport::Usb,
        device_id: Some(device_id),
        serial_number: serial.map(str::to_owned),
        manufacturer: None,
        product: None,
    };
    SerialPortSnapshot {
        stable_id: stable_usb_id(&device),
        suggested_role: role,
        port: SerialPortRef {
            instance_id: PortInstanceId(id),
            device,
        },
        lease: None,
    }
}

fn f9p(id: u64, serial: &str) -> SerialPortSnapshot {
    port(
        id,
        &format!("/dev/ttyACM{id}"),
        Some(serial),
        DeviceRole::RtkBaseStation,
        F9P_USB,
    )
}

fn inventory(ports: Vec<SerialPortSnapshot>) -> SerialInventory {
    SerialInventory {
        revision: InventoryRevision(1),
        health: InventoryHealth::Ready,
        ports,
    }
}

#[test]
fn automatic_linking_is_default_and_pause_and_receiver_settings_round_trip() {
    let mut settings = SerialAutoConnectSettings::default();
    assert!(settings.fc.policy.enabled && settings.rtk.policy.enabled);
    assert!(!settings.fc.policy.paused_by_user && !settings.rtk.policy.paused_by_user);
    settings.rtk.policy.paused_by_user = true;
    settings.rtk.policy.saved = Some(SavedSerialDevice::from_port(&f9p(1, "A").port));
    settings.validate().expect("valid profile");
    let json = serde_json::to_string(&settings).expect("serialize");
    assert_eq!(
        serde_json::from_str::<SerialAutoConnectSettings>(&json).expect("deserialize"),
        settings
    );
}

#[test]
fn saved_device_absence_or_busy_never_falls_back_to_another_receiver() {
    let a = f9p(1, "A");
    let b = f9p(2, "B");
    let mut settings = SerialAutoConnectSettings::default();
    settings.rtk.policy.saved = Some(SavedSerialDevice::from_port(&a.port));
    assert!(select_candidate(
        &inventory(vec![b.clone()]),
        &settings,
        AutoConnectRole::RtkBase,
        None
    )
    .expect("wait")
    .is_none());
    let mut busy = a;
    busy.lease = Some(SerialLeaseStatus {
        id: SerialLeaseId(1),
        owner: SerialOwner::MotorTestTarget,
        phase: LeasePhase::Active,
    });
    assert!(matches!(
        select_candidate(
            &inventory(vec![busy, b]),
            &settings,
            AutoConnectRole::RtkBase,
            None
        ),
        Err(SerialDiscoveryError::PortBusy { .. })
    ));
}

#[test]
fn ambiguity_and_missing_serial_need_selection() {
    let settings = SerialAutoConnectSettings::default();
    for ports in [
        vec![f9p(1, "A"), f9p(2, "B")],
        vec![f9p(1, "duplicate"), f9p(2, "duplicate")],
    ] {
        assert!(matches!(
            select_candidate(&inventory(ports), &settings, AutoConnectRole::RtkBase, None),
            Err(SerialDiscoveryError::AmbiguousDevices { .. })
        ));
    }
    let missing = port(1, "/dev/ttyACM1", None, DeviceRole::RtkBaseStation, F9P_USB);
    assert!(matches!(
        select_candidate(
            &inventory(vec![missing]),
            &settings,
            AutoConnectRole::RtkBase,
            None
        ),
        Err(SerialDiscoveryError::StableIdentityRequired)
    ));
}

#[test]
fn generic_bridges_require_saved_selection_and_m8p_role_does_not_authorize_f9p_setup() {
    let bridge = port(
        1,
        "/dev/ttyUSB1",
        Some("bridge-A"),
        DeviceRole::Other,
        DeviceId {
            vendor_id: 0x0403,
            product_id: 0x6001,
        },
    );
    let m8p = port(
        2,
        "/dev/ttyACM2",
        Some("M8P"),
        DeviceRole::RtkBaseStation,
        DeviceId {
            vendor_id: 0x1546,
            product_id: 0x01a2,
        },
    );
    let inventory = inventory(vec![bridge.clone(), m8p]);
    let mut settings = SerialAutoConnectSettings::default();
    assert!(
        select_candidate(&inventory, &settings, AutoConnectRole::RtkBase, None)
            .expect("no known F9P")
            .is_none()
    );
    settings.rtk.policy.saved = Some(SavedSerialDevice::from_port(&bridge.port));
    assert_eq!(
        select_candidate(&inventory, &settings, AutoConnectRole::RtkBase, None)
            .expect("explicit bridge"),
        Some(bridge.port)
    );
}

#[test]
fn pinned_identity_follows_renumbering_but_not_a_replacement_at_the_old_path() {
    let a = f9p(1, "A");
    let pinned = a.stable_id.clone();
    let mut replacement = f9p(1, "B");
    replacement.port.instance_id = PortInstanceId(3);
    let moved = f9p(4, "A");
    let settings = SerialAutoConnectSettings::default();
    let result = select_candidate(
        &inventory(vec![replacement, moved.clone()]),
        &settings,
        AutoConnectRole::RtkBase,
        pinned.as_ref(),
    )
    .expect("matching USB identity");
    assert_eq!(result, Some(moved.port));
}

#[test]
fn paused_disabled_and_unavailable_inventory_do_not_produce_attempts() {
    let inv = inventory(vec![f9p(1, "A")]);
    let mut settings = SerialAutoConnectSettings::default();
    settings.rtk.policy.paused_by_user = true;
    assert!(
        select_candidate(&inv, &settings, AutoConnectRole::RtkBase, None)
            .expect("paused")
            .is_none()
    );
    settings.rtk.policy.paused_by_user = false;
    settings.rtk.policy.enabled = false;
    assert!(
        select_candidate(&inv, &settings, AutoConnectRole::RtkBase, None)
            .expect("disabled")
            .is_none()
    );
    let mut stale = inv;
    stale.health = InventoryHealth::Unavailable {
        message: "scan failed".into(),
    };
    settings.rtk.policy.enabled = true;
    assert!(matches!(
        select_candidate(&stale, &settings, AutoConnectRole::RtkBase, None),
        Err(SerialDiscoveryError::InventoryUnavailable)
    ));
}

#[test]
fn invalid_versions_bauds_and_passive_auto_baud_are_rejected() {
    let mut settings = SerialAutoConnectSettings {
        version: SERIAL_SETTINGS_VERSION + 1,
        ..Default::default()
    };
    assert!(settings.validate().is_err());
    settings.version = SERIAL_SETTINGS_VERSION;
    settings.fc.baud = BaudSelection::Fixed(0);
    assert!(settings.validate().is_err());
    settings.fc.baud = BaudSelection::Auto;
    settings.rtk.receiver.mode = BaseMode::Preconfigured;
    assert!(settings.validate().is_err());
    settings.rtk.receiver.baud = BaudSelection::Fixed(115_200);
    settings.validate().expect("passive fixed rate");
}

#[test]
fn retry_budget_is_capped_and_configuration_failures_need_user_action() {
    let delays: Vec<_> = (0..8).map(|n| retry_delay(n).as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
    assert_eq!(retry_delay(u32::MAX).as_secs(), 30);
    assert_eq!(
        SerialFailure::discovery(&SerialDiscoveryError::NoAutopilot).retry,
        RetryDisposition::Backoff
    );
    assert_eq!(
        SerialFailure::discovery(&SerialDiscoveryError::PermissionDenied {
            port: "COM3".into()
        })
        .retry,
        RetryDisposition::UserAction
    );
}
