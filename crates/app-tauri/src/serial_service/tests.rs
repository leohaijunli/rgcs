use super::*;
use maggcs_core::devices::{
    DeviceError, DeviceId, PortTransport, SerialDeviceInfo, SerialInventorySource,
};
use std::sync::atomic::AtomicU64;

static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

struct Inventory;
impl SerialInventorySource for Inventory {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError> {
        Ok(vec![SerialDeviceInfo {
            port_name: "/dev/ttyACM900124".into(),
            transport: PortTransport::Usb,
            device_id: Some(DeviceId {
                vendor_id: 0x1546,
                product_id: 0x01a9,
            }),
            serial_number: Some("receiver-A".into()),
            manufacturer: None,
            product: None,
        }])
    }
}

fn directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "maggcs-serial-profile-{}-{}",
        std::process::id(),
        NEXT_TEST.fetch_add(1, Ordering::Relaxed)
    ))
}

async fn service(path: PathBuf, settings: SerialAutoConnectSettings) -> Arc<SerialService> {
    let (discovery, task) = spawn_serial_discovery(
        Arc::new(Inventory),
        SerialDiscoveryConfig {
            poll_interval: Duration::from_secs(60),
            ..Default::default()
        },
    )
    .expect("inventory");
    drop(task);
    discovery.refresh().await.expect("initial inventory");
    Arc::new(SerialService {
        discovery,
        inner: Mutex::new(Inner {
            snapshot: SerialAutoConnectSnapshot {
                revision: revision().expect("revision"),
                settings_revision: revision().expect("revision"),
                settings,
                primary_startup_owner: PrimaryStartupOwner::SerialDiscovery,
                fc: AutoConnectStatus::new(AutoConnectRole::PrimaryFc).expect("FC intent"),
                rtk: AutoConnectStatus::new(AutoConnectRole::RtkBase).expect("base intent"),
                settings_error: None,
            },
            controls: [RoleControl::default(), RoleControl::default()],
        }),
        path: Some(path),
        writer: Arc::new(tokio::sync::Mutex::new(())),
        emit: Box::new(|_| {}),
        wake: Notify::new(),
        closing: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        unreadable_file: AtomicBool::new(false),
        needs_legacy_import: AtomicBool::new(false),
        bootstrap: tokio::sync::Mutex::new(()),
    })
}

#[test]
fn missing_config_defaults_on_and_existing_corrupt_or_newer_files_are_retained() {
    let directory = directory();
    let path = directory.join(SERIAL_SETTINGS_FILE);
    let (settings, error, missing) = load_settings(&path, true);
    assert!(
        missing && error.is_none() && settings.fc.policy.enabled && settings.rtk.policy.enabled
    );
    assert!(
        !load_settings(&path, false).0.fc.policy.enabled,
        "preserve a saved legacy opt-out"
    );
    std::fs::create_dir_all(&directory).expect("directory");
    for text in ["{bad json", r#"{"version":999}"#] {
        std::fs::write(&path, text).expect("fixture");
        let (_, error, missing) = load_settings(&path, true);
        assert!(error.is_some() && !missing);
        assert_eq!(std::fs::read_to_string(&path).expect("retained"), text);
    }
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test]
async fn pause_persists_and_an_old_intent_cannot_pause_a_resumed_role() {
    let directory = directory();
    let path = directory.join(SERIAL_SETTINGS_FILE);
    let service = service(path.clone(), SerialAutoConnectSettings::default()).await;
    let old = service.snapshot().rtk.intent;
    let paused = service
        .pause_memory(AutoConnectRole::RtkBase, Some(&old))
        .expect("pause");
    service.persist(&paused).await.expect("saved pause");
    assert!(load_settings(&path, true).0.rtk.policy.paused_by_user);
    let mut resume = service.snapshot().settings;
    resume.rtk.policy.paused_by_user = false;
    let current = service.save(&paused, resume).await.expect("resume saved");
    assert_ne!(current.rtk.intent, old);
    assert!(service
        .pause_memory(AutoConnectRole::RtkBase, Some(&old))
        .is_err());
    assert!(!service.snapshot().settings.rtk.policy.paused_by_user);
    assert!(!load_settings(&path, true).0.rtk.policy.paused_by_user);
    service.shutdown().await.expect("shutdown");
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test]
async fn concurrent_saves_have_one_winner_and_disk_matches_the_committed_snapshot() {
    let directory = directory();
    let path = directory.join(SERIAL_SETTINGS_FILE);
    let service = service(path.clone(), SerialAutoConnectSettings::default()).await;
    let revision = service.snapshot().settings_revision;
    let mut a = service.snapshot().settings;
    a.fc.policy.enabled = false;
    let mut b = service.snapshot().settings;
    b.rtk.policy.enabled = false;
    let (a, b) = tokio::join!(service.save(&revision, a), service.save(&revision, b));
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(load_settings(&path, true).0, service.snapshot().settings);
    service.shutdown().await.expect("shutdown");
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test]
async fn manual_choice_cancels_and_joins_a_pending_automatic_port_owner() {
    let directory = directory();
    let service = service(
        directory.join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    let port = service.discovery.snapshot().ports[0].port.clone();
    let lease = service
        .discovery
        .reserve(LeaseRequest {
            expected: port.clone(),
            owner: SerialOwner::PrimaryFc,
        })
        .await
        .expect("auto lease");
    let cancel = service.inner.lock().controls[0].cancel.clone();
    service.inner.lock().controls[0].task = Some(tokio::spawn(async move {
        let _lease = lease;
        cancel.cancelled().await;
    }));
    let manual = service
        .begin_manual(AutoConnectRole::PrimaryFc)
        .await
        .expect("manual takes priority");
    manual.check().expect("current manual attempt");
    let lease = service
        .discovery
        .reserve(LeaseRequest {
            expected: port,
            owner: SerialOwner::PrimaryFc,
        })
        .await
        .expect("auto I/O retired first");
    drop(lease);
    drop(manual);
    assert!(
        service.inner.lock().controls[0].blocked,
        "failed manual selection must not fall through to a different auto candidate"
    );
    service.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn failed_manual_replacement_preserves_the_existing_active_session() {
    let service = service(
        directory().join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    let session = SerialSessionRef::Fc {
        connection_id: "42".into(),
        generation: "1".into(),
    };
    service.inner.lock().controls[0].active = Some(session.clone());
    let manual = service
        .begin_manual(AutoConnectRole::PrimaryFc)
        .await
        .expect("manual attempt");
    drop(manual);
    assert_eq!(service.snapshot().fc.session, Some(session));
    assert_eq!(service.snapshot().fc.phase, AutoConnectPhase::Running);
    assert!(!service.inner.lock().controls[0].blocked);
    service.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stale_completion_cannot_replace_the_new_intent_or_session() {
    let service = service(
        directory().join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    let old = service.snapshot().fc.intent;
    let manual = service
        .begin_manual(AutoConnectRole::PrimaryFc)
        .await
        .expect("new intent");
    let before = service.snapshot();
    service.failed(
        &old,
        SerialFailure {
            code: "old".into(),
            message: "obsolete".into(),
            stage: "probe".into(),
            retry: RetryDisposition::Backoff,
        },
    );
    service.running(
        &old,
        SerialSessionRef::Fc {
            connection_id: "1".into(),
            generation: "1".into(),
        },
        None,
        None,
    );
    assert_eq!(service.snapshot(), before);
    drop(manual);
    service.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pause_delivery_does_not_wait_for_a_busy_settings_writer() {
    let directory = directory();
    let service = service(
        directory.join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    let writer = service.writer.clone().lock_owned().await;
    let old_cancel = service.inner.lock().controls[1].cancel.clone();
    let revision = service
        .pause_memory(AutoConnectRole::RtkBase, None)
        .expect("immediate pause");
    assert!(old_cancel.is_cancelled());
    assert!(service.snapshot().settings.rtk.policy.paused_by_user);
    drop(writer);
    service
        .persist(&revision)
        .await
        .expect("persist after releasing writer");
    service.shutdown().await.expect("shutdown");
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test]
async fn legacy_receiver_settings_import_once_before_default_auto_start() {
    let directory = directory();
    let service = service(
        directory.join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    service.needs_legacy_import.store(true, Ordering::Release);
    let receiver = BaseSerialRequest {
        baud: BaudSelection::Fixed(57_600),
        mode: maggcs_core::rtk::base::BaseMode::SurveyIn(maggcs_core::rtk::base::SurveyInConfig {
            accuracy_limit_m: 1.25,
            min_duration_s: 300,
        }),
    };
    service.import_legacy(Some(receiver)).await;
    assert_eq!(service.snapshot().settings.rtk.receiver, receiver);
    assert!(!service.needs_legacy_import.load(Ordering::Acquire));
    service
        .import_legacy(Some(SerialAutoConnectSettings::default().rtk.receiver))
        .await;
    assert_eq!(service.snapshot().settings.rtk.receiver, receiver);
    service.shutdown().await.expect("shutdown");
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test]
async fn automatic_startup_ownership_preserves_manual_and_network_choices() {
    let service = service(
        directory().join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    service.update_startup_owner("udpin:0.0.0.0:14550");
    assert_eq!(
        service.snapshot().primary_startup_owner,
        PrimaryStartupOwner::SerialDiscovery
    );
    service.inner.lock().snapshot.settings.fc.policy.enabled = false;
    service.update_startup_owner("udpin:0.0.0.0:14550");
    assert_eq!(
        service.snapshot().primary_startup_owner,
        PrimaryStartupOwner::LegacyEndpoint
    );
    service.update_startup_owner("serial:/dev/ttyACM0:115200");
    assert_eq!(
        service.snapshot().primary_startup_owner,
        PrimaryStartupOwner::ManualSerialSelection
    );
    service.inner.lock().snapshot.settings_error = Some("unreadable settings".into());
    service.update_startup_owner("udpin:0.0.0.0:14550");
    assert_eq!(
        service.snapshot().primary_startup_owner,
        PrimaryStartupOwner::BlockedSettings
    );
    service.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn changing_rtk_policy_preserves_fc_identity_and_explicit_resume_retries_failed_intents() {
    let directory = directory();
    let service = service(
        directory.join(SERIAL_SETTINGS_FILE),
        SerialAutoConnectSettings::default(),
    )
    .await;
    let old = service.snapshot();
    let pinned = service.discovery.snapshot().ports[0].stable_id.clone();
    service.inner.lock().controls[0].pinned = pinned.clone();
    let mut settings = old.settings;
    settings.rtk.policy.enabled = false;
    let current = service
        .save(&old.settings_revision, settings)
        .await
        .expect("RTK preference");
    assert_eq!(current.fc.intent, old.fc.intent);
    assert_eq!(service.inner.lock().controls[0].pinned, pinned);
    assert_ne!(current.rtk.intent, old.rtk.intent);
    service.failed(
        &current.fc.intent,
        SerialFailure {
            code: "test".into(),
            message: "needs retry".into(),
            stage: "probe".into(),
            retry: RetryDisposition::UserAction,
        },
    );
    let resumed = service
        .save_with_reset(
            &current.settings_revision,
            current.settings,
            Some(AutoConnectRole::PrimaryFc),
        )
        .await
        .expect("explicit retry");
    assert_ne!(resumed.fc.intent, current.fc.intent);
    assert!(!service.inner.lock().controls[0].blocked);
    service.shutdown().await.expect("shutdown");
    std::fs::remove_dir_all(directory).expect("cleanup");
}
