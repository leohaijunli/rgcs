use super::*;
use crate::main_link_service::tests::{wait_until, Harness};
use maggcs_core::mavlink::MavMessage;
use maggcs_core::motor_test::tone::{MotorTone, MotorToneSettings};
use maggcs_core::runtime::{RequestId, Revision};

async fn setup() -> Harness {
    let h = Harness::commands().await;
    h.attach().await;
    wait_until(|| {
        h.state()
            .motor_test
            .target
            .lock()
            .as_ref()
            .is_some_and(|target| target.target.config.lock().is_some())
    })
    .await;
    h
}
async fn deadline<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), future)
        .await
        .expect("owned Motor fixture deadline")
}
fn attach(h: &Harness, window: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .into_iter()
        .find(|target| {
            target
                .roles
                .contains(&maggcs_core::runtime::TargetRole::MotorTest)
        })
        .expect("motor role")
        .target
        .expect("verified");
    h.state()
        .runtime_clients
        .attach(window, "motor-test".into(), target)
        .expect("attach")
        .0
}
fn context(session: &RuntimeClientSession) -> MotorRequestContext {
    MotorRequestContext {
        session: session.clone(),
        request_id: RequestId::fresh().expect("request"),
    }
}
async fn grant(h: &Harness, window: &str, session: &RuntimeClientSession) -> MotorPermissionRef {
    crate::motor_control_service::set_owned(
        h.app.handle(),
        window,
        MotorPermissionRequest {
            context: context(session),
            enabled: true,
            permission: None,
        },
    )
    .await
    .expect("grant")
    .permission
    .expect("permission")
}
fn request(
    h: &Harness,
    session: &RuntimeClientSession,
    permission: &MotorPermissionRef,
) -> MotorStartRequest {
    let revision = h
        .state()
        .motor_test
        .target
        .lock()
        .as_ref()
        .expect("target")
        .target
        .config
        .lock()
        .as_ref()
        .expect("config")
        .revision;
    MotorStartRequest {
        context: context(session),
        permission: permission.clone(),
        config_revision: Revision::new(revision).expect("revision"),
        source: MotorStartSource::Manual { values: vec![0.1] },
    }
}
fn stop_request(session: &RuntimeClientSession, operation: &OperationRef) -> MotorStopRequest {
    MotorStopRequest {
        session: session.clone(),
        operation: operation.clone(),
        request_id: RequestId::fresh().expect("id"),
        emergency: false,
    }
}
fn frames(h: &Harness) -> Vec<::mavlink::common::COMMAND_LONG_DATA> {
    h.trace
        .lock()
        .iter()
        .filter_map(|message| match message {
            MavMessage::COMMAND_LONG(command)
                if command.command == ::mavlink::common::MavCmd::MAV_CMD_ACTUATOR_TEST =>
            {
                Some(command.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn native_permissions_start_updates_and_stop_are_exact_owner_bound() {
    let h = deadline(setup()).await;
    let a = attach(&h, "a");
    let b = attach(&h, "b");
    let permission = deadline(grant(&h, "a", &a)).await;
    let config_revision = request(&h, &a, &permission).config_revision;
    crate::motor_tone_service::set_tones_owned(
        h.app.handle(),
        "a",
        MotorToneSettingsRequest {
            context: context(&a),
            permission: permission.clone(),
            config_revision,
            settings: MotorToneSettings {
                start: MotorTone::Sweep,
                stop: MotorTone::Triple,
            },
        },
    )
    .await
    .expect("owned tone settings");
    assert!(crate::motor_tone_service::set_tones_owned(
        h.app.handle(),
        "b",
        MotorToneSettingsRequest {
            context: context(&b),
            permission: permission.clone(),
            config_revision,
            settings: MotorToneSettings::default(),
        }
    )
    .await
    .is_err());
    assert!(crate::motor_tone_service::set_tones_owned(
        h.app.handle(),
        "a",
        MotorToneSettingsRequest {
            context: context(&a),
            permission: permission.clone(),
            config_revision: Revision::new(config_revision.get() + 1).expect("stale revision"),
            settings: MotorToneSettings::default(),
        }
    )
    .await
    .is_err());
    assert!(deadline(crate::motor_control_service::set_owned(
        h.app.handle(),
        "b",
        MotorPermissionRequest {
            context: context(&b),
            enabled: true,
            permission: None
        }
    ))
    .await
    .is_err());
    assert!(
        deadline(start(h.app.handle(), "b", request(&h, &a, &permission)))
            .await
            .is_err()
    );
    let started = deadline(start(h.app.handle(), "a", request(&h, &a, &permission)))
        .await
        .expect("actual first bank");
    wait_until(|| frames(&h).iter().any(|frame| frame.param2 > 0.0)).await;
    assert!(
        stop(h.app.handle(), "b", stop_request(&b, &started.operation))
            .await
            .is_err()
    );
    let update = MotorValuesRequest {
        session: a.clone(),
        operation: started.operation.clone(),
        request_id: RequestId::fresh().expect("id"),
        revision: Revision::new(1).expect("revision"),
        values: vec![0.3],
    };
    values(h.app.handle(), "a", update.clone()).expect("update");
    assert!(
        values(h.app.handle(), "a", update).is_err(),
        "obsolete update cannot be re-enqueued"
    );
    let receipt = deadline(stop(
        h.app.handle(),
        "a",
        stop_request(&a, &started.operation),
    ))
    .await
    .expect("owned release");
    assert_eq!(receipt.released_motors, 1);
    assert_eq!(receipt.operation, started.operation);
    assert_eq!(
        retained(h.app.handle(), &started.operation)
            .expect("status")
            .cleanup,
        CleanupOutcome::Confirmed
    );
    let second = deadline(start(h.app.handle(), "a", request(&h, &a, &permission)))
        .await
        .expect("fresh operation");
    let old = stop(h.app.handle(), "a", stop_request(&a, &started.operation))
        .await
        .expect("old stop is idle");
    assert_eq!(old.released_motors, 0);
    assert_eq!(
        retained(h.app.handle(), &second.operation)
            .expect("B unaffected")
            .phase,
        maggcs_core::runtime::operations::OperationPhase::Running
    );
    assert!(crate::motor_control_service::set_owned(
        h.app.handle(),
        "b",
        MotorPermissionRequest {
            context: context(&b),
            enabled: false,
            permission: Some(permission.clone()),
        }
    )
    .await
    .is_err());
    crate::motor_control_service::set_owned(
        h.app.handle(),
        "a",
        MotorPermissionRequest {
            context: context(&a),
            enabled: false,
            permission: Some(permission),
        },
    )
    .await
    .expect("owner OFF");
    wait_until(|| {
        retained(h.app.handle(), &second.operation).is_ok_and(|status| {
            status.phase == maggcs_core::runtime::operations::OperationPhase::Finished
        })
    })
    .await;
    assert_eq!(
        retained(h.app.handle(), &second.operation)
            .expect("OFF release")
            .cleanup,
        CleanupOutcome::Confirmed
    );
    assert!(
        !h.handle.is_shutdown_requested(),
        "borrowed main I/O is preserved"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn target_generation_replacement_never_redirects_release_and_retains_uncertainty() {
    let h = deadline(setup()).await;
    let a = attach(&h, "a");
    let permission = deadline(grant(&h, "a", &a)).await;
    let started = deadline(start(h.app.handle(), "a", request(&h, &a, &permission)))
        .await
        .expect("started");
    let releases = frames(&h)
        .iter()
        .filter(|frame| frame.param2 == 0.0)
        .count();
    h.handle.reconnect().await.expect("new generation");
    wait_until(|| {
        retained(h.app.handle(), &started.operation).is_ok_and(|status| {
            status.phase == maggcs_core::runtime::operations::OperationPhase::Finished
        })
    })
    .await;
    let status = retained(h.app.handle(), &started.operation).expect("retained");
    assert_eq!(status.cleanup, CleanupOutcome::Uncertain);
    assert!(matches!(
        status.outcome,
        Some(OperationOutcome::Uncertain { .. })
    ));
    assert_eq!(
        frames(&h)
            .iter()
            .filter(|frame| frame.param2 == 0.0)
            .count(),
        releases,
        "old-generation release is never redirected to the replacement transport"
    );
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn late_cleanup_from_a_closed_motor_window_cannot_stop_or_revoke_a_reopened_owner() {
    let h = deadline(setup()).await;
    let old = attach(&h, "actuator");
    let old_permission = deadline(grant(&h, "actuator", &old)).await;
    let old_start = deadline(start(
        h.app.handle(),
        "actuator",
        request(&h, &old, &old_permission),
    ))
    .await
    .expect("old owner starts");
    wait_until(|| frames(&h).iter().any(|frame| frame.param2 > 0.0)).await;
    h.state()
        .runtime_clients
        .expire_window(&old.owner.client_id)
        .expect("native close");
    wait_until(|| {
        retained(h.app.handle(), &old_start.operation).is_ok_and(|status| {
            status.phase == maggcs_core::runtime::operations::OperationPhase::Finished
        })
    })
    .await;

    let fresh = attach(&h, "actuator");
    assert_ne!(fresh.owner.client_id, old.owner.client_id);
    let fresh_permission = deadline(grant(&h, "actuator", &fresh)).await;
    let fresh_start = deadline(start(
        h.app.handle(),
        "actuator",
        request(&h, &fresh, &fresh_permission),
    ))
    .await
    .expect("reopened owner starts");
    wait_until(|| {
        retained(h.app.handle(), &fresh_start.operation).is_ok_and(|status| {
            status.phase == maggcs_core::runtime::operations::OperationPhase::Running
        })
    })
    .await;
    let releases = frames(&h)
        .iter()
        .filter(|frame| frame.param2 == 0.0)
        .count();

    deadline(crate::motor_operations::stop_all_for_client(
        h.app.handle(),
        &old.owner.client_id,
        Duration::from_secs(2),
    ))
    .await
    .expect("old cleanup stays scoped");
    crate::motor_control_service::revoke_client(&h.state(), &old.owner.client_id);

    assert_eq!(
        retained(h.app.handle(), &fresh_start.operation)
            .expect("reopened status")
            .phase,
        maggcs_core::runtime::operations::OperationPhase::Running,
        "a delayed old-window cleanup cannot finish the replacement operation"
    );
    assert!(
        crate::motor_control_service::status(&h.state()).enabled,
        "the reopened owner keeps its own grant"
    );
    assert_eq!(
        frames(&h)
            .iter()
            .filter(|frame| frame.param2 == 0.0)
            .count(),
        releases,
        "old cleanup sends no release for the replacement output"
    );
    assert!(h.state().runtime_clients.module_attached("motor-test"));

    deadline(crate::motor_operations::stop_all_for_client(
        h.app.handle(),
        &fresh.owner.client_id,
        Duration::from_secs(2),
    ))
    .await
    .expect("replacement can be released exactly");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prepared_owner_loss_is_passive_and_native_close_releases_its_running_operation() {
    let h = deadline(setup()).await;
    let a = attach(&h, "a");
    let permission = deadline(grant(&h, "a", &a)).await;
    let work = begin(h.app.handle(), "a", request(&h, &a, &permission)).expect("prepared");
    h.state()
        .runtime_clients
        .expire_window(&a.owner.client_id)
        .expect("native close");
    let (reply, response) = oneshot::channel();
    work.run(reply).await;
    assert!(response.await.expect("reply").is_err());
    assert!(frames(&h).is_empty());
    let fresh = attach(&h, "a");
    let grant = deadline(grant(&h, "a", &fresh)).await;
    let started = deadline(start(h.app.handle(), "a", request(&h, &fresh, &grant)))
        .await
        .expect("started");
    h.state()
        .runtime_clients
        .expire_window(&fresh.owner.client_id)
        .expect("close");
    wait_until(|| {
        retained(h.app.handle(), &started.operation).is_ok_and(|status| {
            status.phase == maggcs_core::runtime::operations::OperationPhase::Finished
        })
    })
    .await;
    assert_eq!(
        retained(h.app.handle(), &started.operation)
            .expect("retained")
            .cleanup,
        CleanupOutcome::Confirmed
    );
    wait_until(|| frames(&h).iter().any(|frame| frame.param2 == 0.0)).await;
    assert!(!crate::motor_control_service::status(&h.state()).enabled);
    assert!(!h.handle.is_shutdown_requested());
    h.shutdown().await;
}
