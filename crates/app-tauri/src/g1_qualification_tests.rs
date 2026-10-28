use std::sync::Arc;
use std::time::Duration;

use crate::main_link_service::tests::{wait_until, Harness};
use crate::runtime_facade::RuntimeFacade;
use maggcs_core::commands::operation_types::CommandRequestContext;
use maggcs_core::logs::operation_types::LogRequestContext;
use maggcs_core::mission::operation_types::MissionRequestContext;
use maggcs_core::params::operation_types::ParameterRequestContext;
use maggcs_core::runtime::operations::{OperationRequest, OperationResource, RuntimeClientSession};
use maggcs_core::runtime::RequestId;
use maggcs_core::shell::ShellRequestContext;

fn attach(h: &Harness, window: &str, module: &str) -> RuntimeClientSession {
    let target = RuntimeFacade::new(h.app.handle())
        .targets()
        .expect("targets")
        .into_iter()
        .find(|target| {
            target
                .roles
                .contains(&maggcs_core::runtime::TargetRole::Primary)
        })
        .expect("primary")
        .target
        .expect("target");
    h.state()
        .runtime_clients
        .attach(window, module.into(), target)
        .expect("attach")
        .0
}
fn requests(h: &Harness) -> (usize, usize, usize, usize) {
    h.trace
        .lock()
        .iter()
        .fold((0, 0, 0, 0), |mut count, frame| {
            match frame {
                maggcs_core::mavlink::MavMessage::PARAM_REQUEST_READ(_) => count.0 += 1,
                maggcs_core::mavlink::MavMessage::MISSION_REQUEST_LIST(_) => count.1 += 1,
                maggcs_core::mavlink::MavMessage::LOG_REQUEST_LIST(_) => count.2 += 1,
                maggcs_core::mavlink::MavMessage::COMMAND_LONG(command)
                    if command.command
                        == ::mavlink::common::MavCmd::MAV_CMD_PREFLIGHT_REBOOT_SHUTDOWN =>
                {
                    count.3 += 1
                }
                _ => {}
            }
            count
        })
}

async fn assert_pending<T>(task: &mut tauri::async_runtime::JoinHandle<T>) {
    assert!(tokio::time::timeout(Duration::from_millis(30), task)
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn reboot_claim_conflicts_with_every_consequential_adapter_without_writing() {
    let h = Harness::qualification().await;
    let command = attach(&h, "command", "command");
    let resources = [
        OperationResource::ParameterTransfer,
        OperationResource::ParameterWrite,
        OperationResource::ActuatorOutput,
        OperationResource::MissionTransfer,
        OperationResource::Command,
        OperationResource::Shell,
        OperationResource::LogTransfer,
        OperationResource::Capture,
    ];
    for resource in resources {
        let owner = attach(&h, &format!("owner-{resource:?}"), "qualification");
        let lease = h
            .state()
            .runtime_clients
            .operations
            .prepare(OperationRequest {
                request_id: RequestId::fresh().expect("request"),
                owner: owner.owner,
                target: owner.target,
                resources: vec![resource],
                budget_ms: 5000,
            })
            .expect("claim");
        let result = crate::command_operations::begin_reboot(
            h.app.handle(),
            "command",
            CommandRequestContext {
                session: command.clone(),
                request_id: RequestId::fresh().expect("request"),
            },
        );
        assert!(
            matches!(
                result,
                Err(
                    maggcs_core::commands::operation_types::CommandOperationError::Operation {
                        error: maggcs_core::runtime::operations::OperationError::ResourceBusy { .. }
                    }
                )
            ),
            "reboot must conflict with {resource:?}"
        );
        drop(lease);
    }
    assert_eq!(
        requests(&h).3,
        0,
        "arbitration failures have zero reboot writes"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_adapters_keep_owner_cancel_and_target_replacement_isolated() {
    let h = Harness::qualification().await;
    let logs = h
        .state()
        .logs
        .spawn(h.app.handle().clone(), h.handle.clone());
    h.state().set_primary_logs(logs);
    h.hold_reads
        .store(true, std::sync::atomic::Ordering::Release);
    h.mission_peer
        .as_ref()
        .expect("mission")
        .hold_download
        .store(true, std::sync::atomic::Ordering::Release);
    h.log_peer
        .as_ref()
        .expect("logs")
        .hold_list
        .store(true, std::sync::atomic::Ordering::Release);

    let params = attach(&h, "params", "parameters");
    let mission = attach(&h, "mission", "mission");
    let logs_session = attach(&h, "logs", "logs");
    let shell = attach(&h, "shell", "console");
    let foreign = attach(&h, "foreign", "foreign");

    let param_work = crate::params_operations::begin(
        h.app.handle(),
        "params",
        ParameterRequestContext {
            session: params.clone(),
            request_id: RequestId::fresh().expect("request"),
        },
        crate::params_operations::ParameterAction::Read {
            name: "MR_SHARED".into(),
        },
    )
    .expect("param work");
    let param_operation = param_work.operation().clone();
    let mission_work = crate::mission_operations::begin(
        h.app.handle(),
        "mission",
        MissionRequestContext {
            session: mission.clone(),
            request_id: RequestId::fresh().expect("request"),
        },
        crate::mission_operations::MissionAction::Download,
    )
    .expect("mission work");
    let mission_operation = mission_work.operation().clone();
    let log_work = crate::logs_service::begin(
        h.app.handle(),
        "logs",
        LogRequestContext {
            session: logs_session.clone(),
            request_id: RequestId::fresh().expect("request"),
        },
        crate::logs_service::LogAction::List,
        None,
    )
    .expect("log work");
    let log_operation = log_work.operation().clone();

    let console = Arc::new(crate::console_service::ConsoleState::default());
    assert!(
        crate::console_service::open_owned(
            h.app.handle(),
            "shell",
            console,
            ShellRequestContext {
                session: shell,
                request_id: RequestId::fresh().expect("request")
            }
        )
        .await
        .is_err(),
        "Shell cannot overlap the owned Log list"
    );

    let mut param_task = tauri::async_runtime::spawn(param_work.run());
    let mut mission_task = tauri::async_runtime::spawn(mission_work.run());
    let mut log_task = tauri::async_runtime::spawn(log_work.run());
    wait_until(|| {
        let count = requests(&h);
        count.0 > 0 && count.1 > 0 && count.2 > 0
    })
    .await;

    assert!(crate::params_operations::cancel(
        h.app.handle(),
        "foreign",
        &foreign,
        &param_operation
    )
    .is_err());
    assert!(crate::mission_operations::cancel(
        h.app.handle(),
        "foreign",
        &foreign,
        &mission_operation
    )
    .is_err());
    assert!(h
        .state()
        .runtime_clients
        .operations
        .cancel(&foreign.owner, &log_operation)
        .is_err());
    assert_pending(&mut param_task).await;
    assert_pending(&mut mission_task).await;
    assert_pending(&mut log_task).await;

    h.state()
        .runtime_clients
        .expire_window(&params.owner.client_id)
        .expect("native close params");
    assert!(
        tokio::time::timeout(Duration::from_secs(3), &mut param_task)
            .await
            .expect("param retirement")
            .expect("task")
            .is_err()
    );
    assert_pending(&mut mission_task).await;
    assert_pending(&mut log_task).await;

    h.handle
        .reconnect()
        .await
        .expect("replace target generation");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), &mut mission_task)
            .await
            .expect("mission retirement")
            .expect("task")
            .is_err()
    );
    assert!(tokio::time::timeout(Duration::from_secs(5), &mut log_task)
        .await
        .expect("log retirement")
        .expect("task")
        .is_err());
    let retired = requests(&h);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        requests(&h),
        retired,
        "old-generation actors never replay intent after replacement"
    );
    assert!(
        h.state()
            .runtime_clients
            .operations
            .cancel(&params.owner, &param_operation)
            .is_ok(),
        "old exact cancellation is idempotent only for its retained record"
    );
    h.shutdown().await;
}
