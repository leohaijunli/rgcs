use super::super::ubx::tests::{ReceiverControl, TestReceiver};
use super::*;

async fn start_receiver(
    base: &RtkBaseServiceHandle,
    model: &str,
) -> (BaseSessionId, ReceiverControl) {
    let (io, control) = TestReceiver::new(model);
    let mode = BaseMode::SurveyIn(SurveyInConfig::default());
    let id = base
        .start_worker(None, "F9P fixture".into(), mode, move |ctx| {
            run_receiver(Box::new(io), mode, 115_200, ctx)
        })
        .await
        .expect("worker created");
    (id, control)
}

async fn phase(base: &RtkBaseServiceHandle, expected: BasePhase) -> BaseStatus {
    let mut status = base.subscribe();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let current = status.borrow_and_update().clone();
            if current.phase == expected {
                return current;
            }
            status.changed().await.expect("status channel");
        }
    })
    .await
    .expect("phase within budget")
}

#[test]
fn validation_rejects_nonfinite_out_of_range_and_zero_duration() {
    for accuracy in [f64::NAN, f64::INFINITY, -1.0, 0.0, 0.009, 5.001] {
        assert!(SurveyInConfig {
            accuracy_limit_m: accuracy,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    for duration in [0, 361, u32::MAX] {
        assert!(SurveyInConfig {
            min_duration_s: duration,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    SurveyInConfig::default().validate().expect("QGC defaults");
    let id = BaseSessionId(u64::MAX);
    assert_eq!(
        serde_json::to_string(&id).expect("serialize"),
        "\"18446744073709551615\""
    );
    assert_eq!(
        serde_json::from_str::<BaseSessionId>("\"18446744073709551615\"").expect("ID"),
        id
    );
}

#[test]
fn completion_requires_both_targets_and_receiver_completion_flags() {
    let config = SurveyInConfig::default();
    let complete = SurveyInStatus {
        duration_s: 180,
        mean_accuracy_m: 2.0,
        observations: 900,
        valid: true,
        active: false,
    };
    assert!(complete.complete(config));
    for incomplete in [
        SurveyInStatus {
            duration_s: 179,
            ..complete
        },
        SurveyInStatus {
            mean_accuracy_m: 2.0001,
            ..complete
        },
        SurveyInStatus {
            active: true,
            ..complete
        },
        SurveyInStatus {
            valid: false,
            ..complete
        },
    ] {
        assert!(!incomplete.complete(config));
    }
    // Match the actual 0.1 mm threshold written to the receiver, including
    // inputs with more precision than the UBX field. Otherwise a valid survey
    // could wait forever at its rounded receiver limit.
    let precise = SurveyInConfig {
        accuracy_limit_m: 1.23456,
        ..config
    };
    assert_eq!(precise.wire_accuracy_limit().expect("wire limit"), 12_346);
    assert!(SurveyInStatus {
        mean_accuracy_m: 1.2346,
        ..complete
    }
    .complete(precise));
    assert!(!SurveyInStatus {
        mean_accuracy_m: 1.2347,
        ..complete
    }
    .complete(precise));
}

#[tokio::test]
async fn survey_without_vehicle_gates_rtcm_and_old_session_stop_cannot_stop_new_base() {
    let base = RtkBaseServiceHandle::default();
    let mut corrections = base.corrections();
    let (id, control) = start_receiver(&base, "ZED-F9P").await;
    let surveying = phase(&base, BasePhase::Surveying).await;
    assert!(surveying
        .receiver
        .as_deref()
        .is_some_and(|s| s.contains("ZED-F9P")));
    let rtcm = super::super::rtcm::encode_frame(&[0x3e, 0xd0, 1]).expect("1005");
    control.bytes(rtcm.clone());
    control.survey(181, 30_000, false, true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(base.snapshot().phase, BasePhase::Surveying);
    assert!(!control.rtcm_enabled());
    assert!(corrections.try_recv().is_err());
    control.survey(60, 10_000, false, true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(base.snapshot().phase, BasePhase::Surveying);
    control.survey(181, 19_999, true, false);
    phase(&base, BasePhase::Ready).await;
    assert!(control.rtcm_enabled());
    control.bytes(rtcm.clone());
    let correction = tokio::time::timeout(Duration::from_secs(1), corrections.recv())
        .await
        .expect("fresh correction")
        .expect("frame");
    assert_eq!(correction.frame.to_bytes(), rtcm);
    assert_eq!(correction.session_id, id);
    base.stop(id).await.expect("bounded stop");
    assert!(
        correction.cancel.is_cancelled(),
        "pending sends are invalidated at stop"
    );
    base.stop(id).await.expect("idempotent stop");
    let (next, _control) = start_receiver(&base, "ZED-F9P").await;
    assert_ne!(id, next);
    phase(&base, BasePhase::Surveying).await;
    assert!(matches!(base.stop(id).await, Err(BaseError::StaleSession)));
    assert_eq!(base.snapshot().session_id, Some(next));
    base.stop(next).await.expect("stop replacement");
}

#[tokio::test]
async fn unsupported_receiver_never_receives_configuration_writes() {
    let base = RtkBaseServiceHandle::default();
    let (id, control) = start_receiver(&base, "ZED-F9R").await;
    let failure = phase(&base, BasePhase::Failed).await;
    assert!(failure
        .error
        .as_deref()
        .is_some_and(|e| e.contains("not a supported F9P")));
    assert!(control
        .state
        .lock()
        .expect("state")
        .writes
        .iter()
        .all(|b| b[2..4] == [0x0a, 4]));
    base.stop(id).await.expect("release failed worker");
}

#[tokio::test]
async fn stale_survey_validity_cannot_remain_ready_even_while_rtcm_bytes_arrive() {
    let base = RtkBaseServiceHandle::default();
    let (id, control) = start_receiver(&base, "ZED-F9P").await;
    phase(&base, BasePhase::Surveying).await;
    control.survey(181, 19_999, true, false);
    phase(&base, BasePhase::Ready).await;
    let frame = super::super::rtcm::encode_frame(&[0x3e, 0xd0, 1]).expect("RTCM frame");
    let mut status = base.subscribe();
    let failed = tokio::time::timeout(SURVEY_REPORT_TIMEOUT + Duration::from_secs(2), async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {
                _ = tick.tick() => control.bytes(frame.clone()),
                changed = status.changed() => {
                    changed.expect("base status");
                    let current = status.borrow_and_update().clone();
                    if current.phase == BasePhase::Failed { break current; }
                }
            }
        }
    })
    .await
    .expect("survey report freshness bound");
    assert_eq!(failed.failure_kind, Some(BaseFailureKind::Transport));
    assert!(failed
        .error
        .as_deref()
        .is_some_and(|error| error.contains(SURVEY_STATUS_STAGE)));
    assert!(
        failed.seconds_since_data.is_some_and(|age| age < 1.0),
        "raw bytes alone cannot refresh survey validity"
    );
    base.stop(id).await.expect("retire failed source");
}

#[tokio::test]
async fn stop_cancels_an_unanswered_identity_probe_within_one_second() {
    let base = RtkBaseServiceHandle::default();
    let (io, control) = TestReceiver::new("ZED-F9P");
    control.state.lock().expect("state").silent_identity = true;
    let mode = BaseMode::SurveyIn(SurveyInConfig::default());
    let id = base
        .start_worker(None, "silent".into(), mode, move |ctx| {
            run_receiver(Box::new(io), mode, 0, ctx)
        })
        .await
        .expect("start accepted before probe completes");
    tokio::time::sleep(Duration::from_millis(25)).await;
    tokio::time::timeout(Duration::from_secs(1), base.stop(id))
        .await
        .expect("stop deadline")
        .expect("stopped");
    let before = control.state.lock().expect("state").writes.len();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(control.state.lock().expect("state").writes.len(), before);
    assert_eq!(base.snapshot().phase, BasePhase::Stopped);
}

#[tokio::test]
async fn passive_receiver_does_not_write_or_need_vehicle_connection() {
    let base = RtkBaseServiceHandle::default();
    let (io, control) = TestReceiver::new("unknown");
    let id = base
        .start_worker(
            None,
            "passive".into(),
            BaseMode::Preconfigured,
            move |ctx| run_receiver(Box::new(io), BaseMode::Preconfigured, 38_400, ctx),
        )
        .await
        .expect("start passive");
    phase(&base, BasePhase::Ready).await;
    assert!(control.state.lock().expect("state").writes.is_empty());
    base.stop(id).await.expect("stop");
}

#[tokio::test]
async fn invalid_configuration_fails_before_any_serial_open() {
    let base = RtkBaseServiceHandle::default();
    let error = base
        .start_serial(
            SerialSourceConfig {
                port_name: "/dev/ttyUSB99999999".into(),
                baud_rate: 115_200,
            },
            BaseMode::SurveyIn(SurveyInConfig {
                min_duration_s: 0,
                ..Default::default()
            }),
        )
        .await
        .expect_err("invalid input");
    assert!(matches!(error, BaseError::InvalidConfig { .. }));
    assert_eq!(base.snapshot(), BaseStatus::default());
}
