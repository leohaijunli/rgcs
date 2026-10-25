use super::*;
use crate::devices::{DeviceId, DeviceRole, PortTransport};
use std::sync::atomic::AtomicUsize;
use tokio::sync::mpsc;

const TEST_DEADLINE: Duration = Duration::from_secs(1);
const QUIET_POLL: Duration = Duration::from_secs(60);
// Tokio's timer wheel rounds deadlines to a millisecond boundary.
const TIMER_TOLERANCE: Duration = Duration::from_millis(10);

struct ScanRequest {
    reply: std::sync::mpsc::Sender<Result<Vec<SerialDeviceInfo>, DeviceError>>,
}

impl ScanRequest {
    fn complete(self, devices: Vec<SerialDeviceInfo>) {
        self.reply.send(Ok(devices)).expect("scan still waiting");
    }

    fn fail(self) {
        self.reply
            .send(Err(DeviceError::WatcherGone(
                "scripted enumeration failure".into(),
            )))
            .expect("scan still waiting");
    }
}

struct ControlledSource {
    requests: mpsc::UnboundedSender<ScanRequest>,
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl SerialInventorySource for ControlledSource {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError> {
        struct Active<'a>(&'a AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = Active(&self.active);
        self.max_active.fetch_max(count, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (reply, result) = std::sync::mpsc::channel();
        self.requests
            .send(ScanRequest { reply })
            .map_err(|_| DeviceError::WatcherGone("test source closed".into()))?;
        result
            .recv()
            .map_err(|_| DeviceError::WatcherGone("test response dropped".into()))?
    }
}

struct Rig {
    source: Arc<ControlledSource>,
    requests: mpsc::UnboundedReceiver<ScanRequest>,
    handle: SerialDiscoveryHandle,
    task: JoinHandle<()>,
}

impl Rig {
    fn start(poll_interval: Duration) -> Self {
        let (tx, requests) = mpsc::unbounded_channel();
        let source = Arc::new(ControlledSource {
            requests: tx,
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let (handle, task) = spawn_serial_discovery(
            source.clone(),
            SerialDiscoveryConfig {
                poll_interval,
                ..Default::default()
            },
        )
        .expect("spawn discovery");
        Self {
            source,
            requests,
            handle,
            task,
        }
    }

    async fn next(&mut self) -> ScanRequest {
        tokio::time::timeout(TEST_DEADLINE, self.requests.recv())
            .await
            .expect("scan started before deadline")
            .expect("source alive")
    }

    async fn ready(&self) -> SerialInventory {
        let mut state = self.handle.subscribe();
        let result = tokio::time::timeout(
            TEST_DEADLINE,
            state.wait_for(|s| s.health == InventoryHealth::Ready),
        )
        .await
        .expect("ready deadline")
        .expect("worker alive")
        .clone();
        result
    }

    fn request_refresh(&self) -> JoinHandle<Result<SerialInventory, SerialDiscoveryError>> {
        let handle = self.handle.clone();
        tokio::spawn(async move { handle.refresh().await })
    }

    async fn refresh(&mut self, devices: Vec<SerialDeviceInfo>) -> SerialInventory {
        let refresh = self.request_refresh();
        self.next().await.complete(devices);
        tokio::time::timeout(TEST_DEADLINE, refresh)
            .await
            .expect("refresh deadline")
            .expect("refresh task")
            .expect("fresh scan")
    }

    async fn stop(mut self) {
        self.requests.close();
        while let Ok(request) = self.requests.try_recv() {
            drop(request);
        }
        self.handle
            .shutdown(TEST_DEADLINE)
            .await
            .expect("joined shutdown");
        tokio::time::timeout(TEST_DEADLINE, self.task)
            .await
            .expect("join deadline")
            .expect("worker joined");
        assert_eq!(self.source.active.load(Ordering::SeqCst), 0);
    }
}

fn device(port: &str, serial: &str) -> SerialDeviceInfo {
    SerialDeviceInfo {
        port_name: port.into(),
        transport: PortTransport::Usb,
        device_id: Some(DeviceId {
            vendor_id: 0x1546,
            product_id: 0x01a9,
        }),
        serial_number: Some(serial.into()),
        manufacturer: Some("u-blox".into()),
        product: Some("F9P".into()),
    }
}

#[tokio::test]
async fn initial_inventory_is_retained_without_an_added_event_or_refresh() {
    let mut rig = Rig::start(QUIET_POLL);
    let initial = rig.handle.snapshot();
    assert_eq!(initial.health, InventoryHealth::Starting);
    rig.next().await.complete(vec![device("/dev/ttyACM0", "A")]);
    let ready = rig.ready().await;
    assert!(ready.revision > initial.revision);
    assert_eq!(ready.ports.len(), 1);
    assert_eq!(ready.ports[0].suggested_role, DeviceRole::RtkBaseStation);
    assert_eq!(rig.source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*rig.handle.subscribe().borrow(), ready);
    rig.stop().await;
}

#[tokio::test]
async fn failed_initial_scan_recovers_and_later_failure_preserves_attachments() {
    let mut rig = Rig::start(QUIET_POLL);
    let mut state = rig.handle.subscribe();
    rig.next().await.fail();
    tokio::time::timeout(
        TEST_DEADLINE,
        state.wait_for(|s| matches!(s.health, InventoryHealth::Unavailable { .. })),
    )
    .await
    .expect("failure deadline")
    .expect("worker alive");
    assert!(rig.handle.snapshot().ports.is_empty());
    let ready = rig.refresh(vec![device("/dev/ttyACM0", "A")]).await;
    let refresh = rig.request_refresh();
    rig.next().await.fail();
    assert!(matches!(
        refresh.await.expect("refresh"),
        Err(SerialDiscoveryError::Enumeration(_))
    ));
    let failed = rig.handle.snapshot();
    assert!(matches!(failed.health, InventoryHealth::Unavailable { .. }));
    assert_eq!(failed.ports, ready.ports);
    assert!(failed.revision > ready.revision);
    let recovered = rig.refresh(vec![device("/dev/ttyACM0", "A")]).await;
    assert_eq!(recovered.ports, ready.ports);
    assert!(recovered.revision > failed.revision);
    rig.stop().await;
}

#[tokio::test]
async fn reorder_and_duplicate_records_do_not_churn_revisions_or_attachment_ids() {
    let mut rig = Rig::start(QUIET_POLL);
    let a = device("/dev/ttyACM0", "A");
    let b = device("/dev/ttyACM1", "B");
    rig.next().await.complete(vec![b.clone(), a.clone()]);
    let before = rig.ready().await;
    let same = rig.refresh(vec![a.clone(), b, a.clone()]).await;
    assert_eq!(before, same);
    let mut renamed = a;
    renamed.product = Some("updated OS display label".into());
    let changed = rig.refresh(vec![renamed]).await;
    assert_eq!(
        changed.ports[0].port.instance_id,
        before.ports[0].port.instance_id
    );
    assert!(changed.revision > before.revision);
    rig.stop().await;
}

#[tokio::test]
async fn removal_replacement_and_renumbering_allocate_new_attachment_ids() {
    let mut rig = Rig::start(QUIET_POLL);
    let a = device("/dev/ttyACM0", "A");
    rig.next().await.complete(vec![a.clone()]);
    let before = rig.ready().await;
    let empty = rig.refresh(vec![]).await;
    assert!(empty.ports.is_empty());
    let replug = rig.refresh(vec![a.clone()]).await;
    assert_ne!(
        before.ports[0].port.instance_id,
        replug.ports[0].port.instance_id
    );
    let moved = rig
        .refresh(vec![
            device("/dev/ttyACM0", "B"),
            device("/dev/ttyACM2", "A"),
        ])
        .await;
    assert_eq!(moved.ports[1].stable_id, before.ports[0].stable_id);
    for port in &moved.ports {
        assert_ne!(port.port.instance_id, replug.ports[0].port.instance_id);
    }
    rig.stop().await;
}

#[tokio::test]
async fn duplicate_usb_ids_remain_visible_and_non_operator_ports_are_filtered() {
    let mut rig = Rig::start(QUIET_POLL);
    rig.next().await.complete(vec![
        device("/dev/ttyACM0", "duplicate"),
        device("/dev/ttyACM1", "duplicate"),
        device("/dev/ttyS0", "internal"),
        device("/dev/pts/0", "virtual"),
    ]);
    let snapshot = rig.ready().await;
    assert_eq!(snapshot.ports.len(), 2);
    assert_eq!(snapshot.ports[0].stable_id, snapshot.ports[1].stable_id);
    assert_ne!(
        snapshot.ports[0].port.instance_id,
        snapshot.ports[1].port.instance_id
    );
    rig.stop().await;
}

#[tokio::test]
async fn conflicting_metadata_at_one_path_is_not_silently_selected() {
    let mut rig = Rig::start(QUIET_POLL);
    rig.next().await.complete(vec![device("/dev/ttyACM0", "A")]);
    let before = rig.ready().await;
    let refresh = rig.request_refresh();
    rig.next().await.complete(vec![
        device("/dev/ttyACM0", "A"),
        device("/dev/ttyACM0", "B"),
    ]);
    assert!(matches!(
        refresh.await.expect("refresh"),
        Err(SerialDiscoveryError::InvalidInventory { .. })
    ));
    assert_eq!(rig.handle.snapshot().ports, before.ports);
    rig.stop().await;
}

#[tokio::test]
async fn concurrent_refreshes_coalesce_and_wait_for_a_scan_started_after_request() {
    let mut rig = Rig::start(QUIET_POLL);
    let initial = rig.next().await;
    let refreshes: Vec<_> = (0..16).map(|_| rig.request_refresh()).collect();
    tokio::task::yield_now().await;
    initial.complete(vec![device("/dev/ttyACM0", "old")]);
    let fresh_scan = rig.next().await;
    assert!(refreshes.iter().all(|refresh| !refresh.is_finished()));
    fresh_scan.complete(vec![device("/dev/ttyACM1", "fresh")]);
    for refresh in refreshes {
        let snapshot = refresh
            .await
            .expect("refresh task")
            .expect("refresh result");
        assert_eq!(
            snapshot.ports[0].port.device.serial_number.as_deref(),
            Some("fresh")
        );
    }
    assert_eq!(rig.source.calls.load(Ordering::SeqCst), 2);
    assert_eq!(rig.source.max_active.load(Ordering::SeqCst), 1);
    rig.stop().await;
}

#[tokio::test]
async fn periodic_snapshots_recover_coalesced_hotplug_updates_for_a_slow_subscriber() {
    let mut rig = Rig::start(Duration::from_millis(40));
    rig.next().await.complete(vec![device("/dev/ttyACM0", "A")]);
    let first = rig.ready().await;
    let mut slow = rig.handle.subscribe();
    slow.borrow_and_update();
    rig.next().await.complete(vec![]);
    // Receiving the next call is a barrier: the empty result was processed.
    rig.next().await.complete(vec![device("/dev/ttyACM2", "A")]);
    let mut latest = rig.handle.subscribe();
    let last = tokio::time::timeout(
        TEST_DEADLINE,
        latest.wait_for(|s| {
            s.ports
                .first()
                .is_some_and(|p| p.port.device.port_name == "/dev/ttyACM2")
        }),
    )
    .await
    .expect("periodic update")
    .expect("worker alive")
    .clone();
    assert!(slow.has_changed().expect("worker alive"));
    assert_eq!(*slow.borrow_and_update(), last);
    assert!(last.revision > first.revision);
    assert_ne!(
        last.ports[0].port.instance_id,
        first.ports[0].port.instance_id
    );
    rig.stop().await;
}

#[tokio::test]
async fn timed_out_scan_is_drained_and_its_late_result_is_discarded() {
    let mut rig = Rig::start(Duration::from_millis(40));
    rig.next().await.complete(vec![device("/dev/ttyACM0", "A")]);
    let before = rig.ready().await;
    let refresh = rig.request_refresh();
    let blocked = rig.next().await;
    tokio::time::pause();
    tokio::time::advance(DEFAULT_ENUMERATION_TIMEOUT + TIMER_TOLERANCE).await;
    assert!(matches!(
        refresh.await.expect("refresh"),
        Err(SerialDiscoveryError::EnumerationTimeout)
    ));
    assert!(matches!(
        rig.handle.refresh().await,
        Err(SerialDiscoveryError::EnumerationTimeout)
    ));
    assert_eq!(rig.handle.snapshot().ports, before.ports);
    assert_eq!(rig.source.active.load(Ordering::SeqCst), 1);
    assert_eq!(rig.source.max_active.load(Ordering::SeqCst), 1);
    tokio::time::resume();
    blocked.complete(vec![device("/dev/ttyACM1", "late")]);
    let recovery = rig.next().await;
    // The next call can start only after draining the timed-out call. Its
    // result must still not have become current, even briefly before recovery.
    assert_eq!(rig.handle.snapshot().ports, before.ports);
    assert!(matches!(
        rig.handle.snapshot().health,
        InventoryHealth::Unavailable { .. }
    ));
    recovery.complete(vec![device("/dev/ttyACM2", "current")]);
    let recovered = rig.ready().await;
    assert_eq!(
        recovered.ports[0].port.device.serial_number.as_deref(),
        Some("current")
    );
    assert_eq!(rig.source.max_active.load(Ordering::SeqCst), 1);
    rig.stop().await;
}

#[tokio::test]
async fn shutdown_timeout_retains_inflight_scan_until_actual_exit() {
    let mut rig = Rig::start(QUIET_POLL);
    let blocked = rig.next().await;
    tokio::time::pause();
    let handle = rig.handle.clone();
    let stopping = tokio::spawn(async move { handle.shutdown(Duration::from_millis(10)).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(10) + TIMER_TOLERANCE).await;
    assert!(matches!(
        stopping.await.expect("stop"),
        Err(SerialDiscoveryError::StopTimeout)
    ));
    assert!(matches!(
        rig.handle.refresh().await,
        Err(SerialDiscoveryError::Cancelled)
    ));
    assert!(!rig.task.is_finished());
    assert_eq!(rig.source.active.load(Ordering::SeqCst), 1);
    tokio::time::resume();
    blocked.complete(vec![device("/dev/ttyACM0", "too late")]);
    rig.handle
        .shutdown(TEST_DEADLINE)
        .await
        .expect("actual exit");
    rig.handle
        .shutdown(Duration::ZERO)
        .await
        .expect("idempotent");
    assert!(rig.handle.snapshot().ports.is_empty());
    assert!(matches!(
        rig.handle.snapshot().health,
        InventoryHealth::Unavailable { .. }
    ));
    assert_eq!(rig.source.calls.load(Ordering::SeqCst), 1);
    rig.stop().await;
}

#[tokio::test]
async fn dropping_last_handle_stops_even_with_snapshot_subscribers() {
    let mut rig = Rig::start(QUIET_POLL);
    let blocked = rig.next().await;
    let mut snapshots = rig.handle.subscribe();
    drop(rig.handle);
    tokio::time::timeout(
        TEST_DEADLINE,
        snapshots.wait_for(|s| matches!(s.health, InventoryHealth::Unavailable { .. })),
    )
    .await
    .expect("drop cancellation")
    .expect("draining source");
    assert!(!rig.task.is_finished());
    blocked.complete(vec![]);
    tokio::time::timeout(TEST_DEADLINE, rig.task)
        .await
        .expect("join deadline")
        .expect("join");
    assert_eq!(rig.source.active.load(Ordering::SeqCst), 0);
    assert!(snapshots.has_changed().is_err());
}

#[tokio::test]
async fn aborted_worker_reports_failure_rather_than_successful_shutdown() {
    let mut rig = Rig::start(QUIET_POLL);
    let blocked = rig.next().await;
    rig.task.abort();
    assert!(rig.task.await.expect_err("aborted").is_cancelled());
    assert!(matches!(
        rig.handle.refresh().await,
        Err(SerialDiscoveryError::WorkerGone)
    ));
    assert!(matches!(
        rig.handle.shutdown(TEST_DEADLINE).await,
        Err(SerialDiscoveryError::WorkerGone)
    ));
    blocked.complete(vec![]);
}

#[test]
fn stable_identity_requires_usb_vid_pid_and_a_nonblank_case_sensitive_serial() {
    let original = device("/dev/ttyACM0", "ABC");
    let mut changed = original.clone();
    changed.port_name = "/dev/ttyACM9".into();
    changed.product = None;
    changed.manufacturer = None;
    assert_eq!(stable_usb_id(&original), stable_usb_id(&changed));
    changed.serial_number = Some("abc".into());
    assert_ne!(stable_usb_id(&original), stable_usb_id(&changed));
    for serial in [None, Some("".into()), Some(" \t".into())] {
        changed.serial_number = serial;
        assert_eq!(stable_usb_id(&changed), None);
    }
    changed = original.clone();
    changed.device_id = None;
    assert_eq!(stable_usb_id(&changed), None);
    changed = original;
    changed.transport = PortTransport::Unknown;
    assert_eq!(stable_usb_id(&changed), None);
}

#[test]
fn opaque_ids_preserve_values_above_javascript_integer_precision_and_reject_invalid_wire_types() {
    macro_rules! check {
        ($ty:ty) => {{
            for value in [9_007_199_254_740_993_u64, u64::MAX] {
                let wire = format!("\"{value}\"");
                let id: $ty = serde_json::from_str(&wire).expect("lossless ID");
                assert_eq!(serde_json::to_string(&id).expect("serialize"), wire);
            }
            for invalid in ["1", "null", "\"0\"", "\"-1\"", "\"18446744073709551616\""] {
                assert!(serde_json::from_str::<$ty>(invalid).is_err(), "{invalid}");
            }
        }};
    }
    check!(InventoryRevision);
    check!(PortInstanceId);
    check!(SerialLeaseId);
}

#[test]
fn ids_fail_instead_of_wrapping_and_invalid_timer_config_is_rejected_before_spawn() {
    let exhausted = AtomicU64::new(u64::MAX);
    assert!(matches!(
        next_id(&exhausted),
        Err(SerialDiscoveryError::IdExhausted)
    ));
    assert_eq!(exhausted.load(Ordering::Relaxed), u64::MAX);
    for (poll_interval, enumeration_timeout) in [
        (Duration::ZERO, DEFAULT_ENUMERATION_TIMEOUT),
        (QUIET_POLL, Duration::ZERO),
        (Duration::MAX, DEFAULT_ENUMERATION_TIMEOUT),
    ] {
        let result = spawn_serial_discovery(
            Arc::new(DeviceManager::new(DeviceDatabase::builtin())),
            SerialDiscoveryConfig {
                poll_interval,
                enumeration_timeout,
            },
        );
        assert!(matches!(
            result,
            Err(SerialDiscoveryError::InvalidConfig { .. })
        ));
    }
}
