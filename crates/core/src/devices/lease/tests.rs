use super::*;
use crate::devices::{
    spawn_serial_discovery, DeviceError, DeviceId, PortTransport, SerialDeviceInfo,
    SerialDiscoveryConfig, SerialInventorySource,
};
use std::time::Duration;
use tokio::task::JoinHandle;

const DEADLINE: Duration = Duration::from_secs(1);

struct Source(Mutex<Vec<SerialDeviceInfo>>);
impl SerialInventorySource for Source {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError> {
        Ok(self.0.lock().expect("test source").clone())
    }
}

fn device(path: &str, serial: &str) -> SerialDeviceInfo {
    SerialDeviceInfo {
        port_name: path.into(),
        transport: PortTransport::Usb,
        device_id: Some(DeviceId {
            vendor_id: 0x1546,
            product_id: 0x01a9,
        }),
        serial_number: Some(serial.into()),
        manufacturer: None,
        product: None,
    }
}

async fn setup(
    devices: Vec<SerialDeviceInfo>,
) -> (Arc<Source>, SerialDiscoveryHandle, JoinHandle<()>) {
    let source = Arc::new(Source(Mutex::new(devices)));
    let (handle, task) = spawn_serial_discovery(
        source.clone(),
        SerialDiscoveryConfig {
            poll_interval: Duration::from_secs(60),
            ..Default::default()
        },
    )
    .expect("discovery");
    handle.refresh().await.expect("initial scan");
    (source, handle, task)
}

async fn stop(handle: SerialDiscoveryHandle, task: JoinHandle<()>) {
    handle
        .shutdown(DEADLINE)
        .await
        .expect("stop without leases");
    tokio::time::timeout(DEADLINE, task)
        .await
        .expect("join deadline")
        .expect("joined");
}

#[tokio::test]
async fn all_roles_race_for_one_reservation_and_publish_the_winner() {
    let (_, handle, task) = setup(vec![device("/dev/ttyACM9000", "A")]).await;
    let selected = handle.snapshot().ports[0].port.clone();
    let attempts: Vec<_> = [
        SerialOwner::PrimaryFc,
        SerialOwner::MotorTestTarget,
        SerialOwner::AuxiliarySource,
        SerialOwner::RtkBase,
    ]
    .into_iter()
    .map(|owner| {
        let handle = handle.clone();
        let expected = selected.clone();
        tokio::spawn(async move { handle.reserve(LeaseRequest { expected, owner }).await })
    })
    .collect();
    let mut winner = None;
    let mut losers = 0;
    for attempt in attempts {
        match attempt.await.expect("attempt") {
            Ok(lease) => {
                assert!(winner.is_none());
                winner = Some(lease);
            }
            Err(SerialDiscoveryError::PortBusy { owner: Some(_) }) => losers += 1,
            other => panic!("unexpected reserve result: {other:?}"),
        }
    }
    assert_eq!(losers, 3);
    let lease = winner.expect("one owner");
    assert_eq!(
        handle.snapshot().ports[0].lease.as_ref().map(|s| s.id),
        Some(lease.id())
    );
    drop(lease);
    assert!(handle.snapshot().ports[0].lease.is_none());
    stop(handle, task).await;
}

#[tokio::test]
async fn phase_updates_survive_refresh_and_release_is_immediately_visible() {
    let (_, handle, task) = setup(vec![device("/dev/ttyACM9000", "A")]).await;
    let expected = handle.snapshot().ports[0].port.clone();
    let lease = handle
        .reserve(LeaseRequest {
            expected,
            owner: SerialOwner::RtkBase,
        })
        .await
        .expect("lease");
    lease.set_phase(LeasePhase::Active).expect("active");
    let active = handle.refresh().await.expect("scan");
    assert_eq!(
        active.ports[0].lease.as_ref().map(|s| s.phase),
        Some(LeasePhase::Active)
    );
    lease.set_phase(LeasePhase::Stopping).expect("stopping");
    assert!(handle.snapshot().revision > active.revision);
    drop(lease);
    assert!(handle.snapshot().ports[0].lease.is_none());
    stop(handle, task).await;
}

#[tokio::test]
async fn replacement_cannot_be_selected_or_opened_while_old_worker_owns_the_path() {
    let (source, handle, task) = setup(vec![device("/dev/ttyACM9000", "A")]).await;
    let old = handle.snapshot().ports[0].port.clone();
    let lease = handle
        .reserve(LeaseRequest {
            expected: old.clone(),
            owner: SerialOwner::PrimaryFc,
        })
        .await
        .expect("lease");
    *source.0.lock().expect("source") = vec![device("/dev/ttyACM9000", "B")];
    assert!(matches!(
        lease.revalidate().await,
        Err(SerialDiscoveryError::DeviceChanged { .. })
    ));
    assert!(matches!(
        handle
            .reserve(LeaseRequest {
                expected: old,
                owner: SerialOwner::RtkBase
            })
            .await,
        Err(SerialDiscoveryError::DeviceChanged { .. })
    ));
    let replacement = handle.snapshot().ports[0].port.clone();
    assert!(matches!(
        handle
            .reserve(LeaseRequest {
                expected: replacement.clone(),
                owner: SerialOwner::RtkBase
            })
            .await,
        Err(SerialDiscoveryError::PortBusy { .. })
    ));
    drop(lease);
    let replacement = handle
        .reserve(LeaseRequest {
            expected: replacement,
            owner: SerialOwner::RtkBase,
        })
        .await
        .expect("after actual release");
    drop(replacement);
    stop(handle, task).await;
}

#[tokio::test]
async fn duplicate_stable_keys_cannot_acquire_two_workers() {
    let (_, handle, task) = setup(vec![
        device("/dev/ttyACM9000", "duplicate"),
        device("/dev/ttyACM9001", "duplicate"),
    ])
    .await;
    let ports = handle.snapshot().ports;
    let lease = handle
        .reserve(LeaseRequest {
            expected: ports[0].port.clone(),
            owner: SerialOwner::RtkBase,
        })
        .await
        .expect("first interface");
    assert!(handle.snapshot().ports.iter().all(|p| p.lease.is_some()));
    assert!(matches!(
        handle
            .reserve(LeaseRequest {
                expected: ports[1].port.clone(),
                owner: SerialOwner::PrimaryFc
            })
            .await,
        Err(SerialDiscoveryError::PortBusy { .. })
    ));
    drop(lease);
    stop(handle, task).await;
}

#[tokio::test]
async fn a_new_attachment_needs_a_new_lease_even_when_usb_identity_matches() {
    let (source, handle, task) = setup(vec![device("/dev/ttyACM9000", "A")]).await;
    let lease = handle
        .reserve(LeaseRequest {
            expected: handle.snapshot().ports[0].port.clone(),
            owner: SerialOwner::PrimaryFc,
        })
        .await
        .expect("lease");
    source.0.lock().expect("source").clear();
    assert!(matches!(
        lease.revalidate().await,
        Err(SerialDiscoveryError::DeviceGone { .. })
    ));
    *source.0.lock().expect("source") = vec![device("/dev/ttyACM9000", "A")];
    assert!(matches!(
        lease.revalidate().await,
        Err(SerialDiscoveryError::DeviceChanged { .. })
    ));
    drop(lease);
    stop(handle, task).await;
}

#[tokio::test]
async fn shutdown_waits_for_the_actual_lease_owner_and_can_be_retried() {
    let (_, handle, task) = setup(vec![device("/dev/ttyACM9000", "A")]).await;
    let lease = handle
        .reserve(LeaseRequest {
            expected: handle.snapshot().ports[0].port.clone(),
            owner: SerialOwner::RtkBase,
        })
        .await
        .expect("lease");
    assert!(matches!(
        handle.shutdown(Duration::from_millis(10)).await,
        Err(SerialDiscoveryError::StopTimeout)
    ));
    assert!(!task.is_finished());
    assert!(handle.snapshot().ports[0].lease.is_some());
    assert!(matches!(
        lease.revalidate().await,
        Err(SerialDiscoveryError::Cancelled)
    ));
    drop(lease);
    stop(handle, task).await;
}

#[tokio::test]
async fn lease_retains_discovery_until_the_io_owner_drops() {
    let (_, handle, task) = setup(vec![device("/dev/ttyACM9000", "A")]).await;
    let lease = handle
        .reserve(LeaseRequest {
            expected: handle.snapshot().ports[0].port.clone(),
            owner: SerialOwner::RtkBase,
        })
        .await
        .expect("lease");
    drop(handle);
    lease.revalidate().await.expect("service retained");
    assert!(!task.is_finished());
    drop(lease);
    tokio::time::timeout(DEADLINE, task)
        .await
        .expect("last owner stop")
        .expect("joined");
}

#[test]
fn windows_com_aliases_are_normalized() {
    assert_eq!(
        canonical_serial_path(Path::new("com12")),
        PathBuf::from("COM12")
    );
    assert_eq!(
        canonical_serial_path(Path::new(r"\\.\COM12")),
        PathBuf::from("COM12")
    );
    assert_ne!(
        canonical_serial_path(Path::new("COM12")),
        canonical_serial_path(Path::new("COM13"))
    );
}

#[cfg(unix)]
#[test]
fn live_unix_symlinks_resolve_to_the_same_key() {
    let path = std::env::temp_dir().join(format!("maggcs-serial-alias-{}", std::process::id()));
    std::fs::create_dir_all(&path).expect("test directory");
    let target = path.join("port");
    let alias = path.join("alias");
    std::fs::write(&target, []).expect("test file");
    std::os::unix::fs::symlink(&target, &alias).expect("test alias");
    assert_eq!(
        canonical_serial_path(&target),
        canonical_serial_path(&alias)
    );
    std::fs::remove_dir_all(path).expect("cleanup");
}
