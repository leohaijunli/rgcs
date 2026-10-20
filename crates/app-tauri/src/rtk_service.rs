//! RTK injection bridge (R4): per-link service runtime and Tauri command adapters.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Runtime, State};

use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::MessageRoute;
use maggcs_core::rtk::multi::{ForeignInjector, ForeignState, FOREIGN_WINDOW};
use maggcs_core::rtk::service::{RtkServiceHandle, RtkStats};

use crate::state::AppState;

/// Process-wide snapshots shown immediately when the RTK drawer opens.
#[derive(Default)]
pub struct RtkState {
    last: StdMutex<Option<RtkStats>>,
    foreign: StdMutex<Option<ForeignState>>,
}

/// RTK service, UI stats forwarder and foreign-injector observer for one link.
pub struct RtkRuntime {
    state: Arc<RtkState>,
    service: RtkServiceHandle,
    service_task: Option<tokio::task::JoinHandle<()>>,
    stats_task: Option<tauri::async_runtime::JoinHandle<()>>,
    foreign_stop: Option<tokio::sync::watch::Sender<bool>>,
    foreign_task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl RtkRuntime {
    /// Create all RTK tasks on the specified link.
    pub fn spawn<R: Runtime>(
        state: Arc<RtkState>,
        app: AppHandle<R>,
        conn: ConnectionHandle,
    ) -> Self {
        let service_conn = conn.clone();
        let (service, mut stats_rx, service_task) =
            maggcs_core::rtk::service::spawn_with_task(service_conn);
        let stats_state = Arc::clone(&state);
        let stats_app = app.clone();
        let stats_task = tauri::async_runtime::spawn(async move {
            while let Some(stats) = stats_rx.recv().await {
                *stats_state.last.lock().expect("rtk stats lock poisoned") = Some(stats.clone());
                if stats_app.emit("rtk_stats", &stats).is_err() {
                    return;
                }
            }
        });

        let (foreign_stop, mut stop_rx) = tokio::sync::watch::channel(false);
        let foreign_state = Arc::clone(&state);
        let foreign_task = tauri::async_runtime::spawn(async move {
            use ::mavlink::MessageData;
            let mut events = conn.subscribe_route(MessageRoute::messages(&[
                ::mavlink::common::GPS_RTCM_DATA_DATA::ID,
            ]));
            let mut injector = ForeignInjector::new();
            let mut last = ForeignState::default();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => return,
                    ev = events.recv() => match ev {
                        Ok(ConnectionEvent::Message(_)) => injector.observe(Instant::now()),
                        Ok(_) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                    },
                    _ = tick.tick() => {},
                }
                let now = Instant::now();
                let current = ForeignState {
                    active: injector.active(now, FOREIGN_WINDOW),
                    packets: injector.packets(),
                };
                if current == last {
                    continue;
                }
                last = current;
                *foreign_state
                    .foreign
                    .lock()
                    .expect("rtk foreign lock poisoned") = Some(current);
                if app.emit("rtk_foreign", current).is_err() {
                    return;
                }
            }
        });

        Self {
            state,
            service,
            service_task: Some(service_task),
            stats_task: Some(stats_task),
            foreign_stop: Some(foreign_stop),
            foreign_task: Some(foreign_task),
        }
    }

    pub(crate) fn service(&self) -> RtkServiceHandle {
        self.service.clone()
    }

    /// Stop the RTCM source and observer and join all per-link tasks.
    pub async fn stop(mut self, deadline: Duration) -> StopOutcome {
        let started = Instant::now();
        if let Some(stop) = self.foreign_stop.take() {
            drop(stop);
        }
        let mut timed_out = false;
        let mut failures = Vec::new();
        if let Some(mut task) = self.foreign_task.take() {
            match tokio::time::timeout(deadline.saturating_sub(started.elapsed()), &mut task).await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("RTK foreign observer failed: {error}")),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    timed_out = true;
                }
            }
        }
        self.state
            .last
            .lock()
            .expect("rtk stats lock poisoned")
            .take();
        self.state
            .foreign
            .lock()
            .expect("rtk foreign lock poisoned")
            .take();
        let shutdown = tokio::time::timeout(
            deadline.saturating_sub(started.elapsed()),
            self.service.shutdown(),
        )
        .await;
        match shutdown {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(format!("RTK service shutdown failed: {error}")),
            Err(_) => {
                if let Some(task) = self.service_task.as_mut() {
                    task.abort();
                    let _ = task.await;
                }
                return StopOutcome::TimedOut {
                    elapsed_ms: elapsed_ms(started),
                };
            }
        }
        let Some(mut task) = self.service_task.take() else {
            return StopOutcome::NotRunning;
        };
        let task_outcome =
            match tokio::time::timeout(deadline.saturating_sub(started.elapsed()), &mut task).await
            {
                Ok(Ok(())) if timed_out => StopOutcome::TimedOut {
                    elapsed_ms: elapsed_ms(started),
                },
                Ok(Ok(())) if !failures.is_empty() => StopOutcome::Failed {
                    message: failures.join("; "),
                },
                Ok(Ok(())) => StopOutcome::Stopped {
                    elapsed_ms: elapsed_ms(started),
                },
                Ok(Err(error)) => StopOutcome::Failed {
                    message: format!("{}; RTK service task failed: {error}", failures.join("; ")),
                },
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    StopOutcome::TimedOut {
                        elapsed_ms: elapsed_ms(started),
                    }
                }
            };
        let stats_outcome = if let Some(mut stats_task) = self.stats_task.take() {
            match tokio::time::timeout(deadline.saturating_sub(started.elapsed()), &mut stats_task)
                .await
            {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("RTK stats task failed: {error}")),
                Err(_) => {
                    stats_task.abort();
                    let _ = stats_task.await;
                    return StopOutcome::TimedOut {
                        elapsed_ms: elapsed_ms(started),
                    };
                }
            }
        } else {
            None
        };
        match (task_outcome, stats_outcome) {
            (StopOutcome::Stopped { .. }, None) => StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            },
            (StopOutcome::Stopped { .. }, Some(message)) => StopOutcome::Failed { message },
            (outcome, Some(message)) => StopOutcome::Failed {
                message: format!("{outcome:?}; {message}"),
            },
            (outcome, None) => outcome,
        }
    }
}

impl Drop for RtkRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.service_task {
            task.abort();
        }
        if let Some(task) = &self.stats_task {
            task.abort();
        }
        if let Some(task) = &self.foreign_task {
            task.abort();
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// Start injecting RTCM from the configured F9P serial port.
#[tauri::command]
pub async fn rtk_start_serial(
    port_name: String,
    baud_rate: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let handle = state.primary_rtk().ok_or("not connected")?;
    handle
        .start_serial(maggcs_core::rtk::source::SerialSourceConfig {
            port_name,
            baud_rate,
        })
        .await
        .map_err(|e| e.to_string())
}

/// Stop the active injection source (idempotent).
#[tauri::command]
pub async fn rtk_stop(state: State<'_, AppState>) -> Result<(), String> {
    let handle = state.primary_rtk().ok_or("not connected")?;
    handle.stop().await.map_err(|e| e.to_string())
}

/// Last stats snapshot (a freshly opened drawer shows it immediately).
#[tauri::command]
pub fn rtk_last_stats(state: State<'_, AppState>) -> Option<RtkStats> {
    state
        .rtk
        .last
        .lock()
        .expect("rtk stats lock poisoned")
        .clone()
}

/// Last foreign-injector snapshot (N4): seeds the drawer before the first event.
#[tauri::command]
pub fn rtk_foreign_state(state: State<'_, AppState>) -> Option<ForeignState> {
    *state.rtk.foreign.lock().expect("rtk foreign lock poisoned")
}
