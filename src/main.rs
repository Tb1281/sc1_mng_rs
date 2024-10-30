#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::time::sleep;
use ui::{
    run_event_loop, ComponentHandle, GlobalCallbacks, GlobalState, MainWindow, Model, SharedString,
    VecModel,
};
use utils::{
    check_valid, get_mutex, get_path, process_handles, query_child, run_sc1, save_log, to_log,
    SC1ProcessInfo,
};
use windows::Win32::System::Threading::TerminateProcess;

mod settings;
mod utils;

#[tokio::main]
async fn main() -> Result<()> {
    if !get_mutex() {
        return Ok(());
    }

    let app = MainWindow::new()?;
    let childs: Arc<Mutex<Vec<SC1ProcessInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let callbacks = app.global::<GlobalCallbacks>();

    callbacks.on_browse_32({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                ui::spawn_local({
                    let app = app.clone();
                    async move {
                        let state = app.global::<GlobalState>();
                        state.set_path_32(
                            tokio::task::spawn_blocking(|| get_path().unwrap_or_default())
                                .await
                                .unwrap()
                                .into(),
                        );
                    }
                })
                .ok();
            }
        }
    });

    callbacks.on_browse_64({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                ui::spawn_local({
                    let app = app.clone();
                    async move {
                        let state = app.global::<GlobalState>();
                        state.set_path_64(
                            tokio::task::spawn_blocking(|| get_path().unwrap_or_default())
                                .await
                                .unwrap()
                                .into(),
                        );
                    }
                })
                .ok();
            }
        }
    });

    callbacks.on_run_game({
        let childs = childs.clone();
        let app = app.as_weak();
        move |path| {
            if let Some(app) = app.upgrade() {
                let state = app.global::<GlobalState>();
                let logs = state.get_logs();
                let new_log = if let Some(child) = run_sc1(&path, &vec!["-launch"]) {
                    let pid = child.pid;
                    tokio::spawn({
                        let childs = childs.clone();
                        async move {
                            let mut childs = childs.lock().await;
                            childs.push(child);
                        }
                    });
                    to_log(&format!("Created new StarCrate.exe(PID: {})", pid)).into()
                } else {
                    to_log("Error: Cannot Launch StarCrate.exe").into()
                };
                if let Some(vec) = logs.as_any().downcast_ref::<VecModel<SharedString>>() {
                    vec.push(new_log);
                    state.set_logs(logs);
                }
            }
        }
    });

    callbacks.on_settings_update({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                settings::save_settings(&app);
            }
        }
    });

    callbacks.on_save_logs({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                let state = app.global::<GlobalState>();
                let logs = state.get_logs();
                if let Some(vec) = logs.as_any().downcast_ref::<VecModel<SharedString>>() {
                    let vec: Vec<String> = vec.iter().map(Into::into).collect();
                    save_log(&vec);
                }
            }
        }
    });

    callbacks.on_kill_all({
        let app = app.as_weak();
        let childs = childs.clone();
        move || {
            if let Some(_) = app.upgrade() {
                tokio::spawn({
                    let childs = childs.clone();
                    async move {
                        let mut childs = childs.lock().await;
                        childs.iter_mut().for_each(|child| {
                            if unsafe { TerminateProcess(*child.handle, 0) }.is_ok() {
                                child.set_terminated();
                            }
                        });
                    }
                });
            }
        }
    });

    ui::spawn_local({
        let app = app.clone();
        async move {
            let state = app.global::<GlobalState>();
            settings::apply_saved_settings(&state).await;
            loop {
                let mut childs = childs.lock().await;
                let logs = state.get_logs();
                let vec = logs.as_any().downcast_ref::<VecModel<SharedString>>();
                let mut log_chaged = false;

                let mut new_childs = process_handles();
                new_childs.retain(|child| !childs.contains(child));
                childs.extend(new_childs);

                let prev = childs.len();
                childs.retain(|child| !child.get_terminated());
                let terminated = prev - childs.len();
                match vec {
                    Some(vec) if terminated > 0 => {
                        vec.push(
                            to_log(&format!(
                                "Successfully Terminated {} Instance(s)",
                                terminated
                            ))
                            .into(),
                        );
                        log_chaged = true;
                    }
                    _ => (),
                }

                childs.retain(|child| {
                    if !check_valid(&child.handle) {
                        if child.get_processed() {
                            if let Some(vec) = vec {
                                vec.push(
                                    to_log(&format!(
                                        "StarCraft.exe(PID: {}) is Terminated",
                                        child.pid
                                    ))
                                    .into(),
                                );
                                log_chaged = true;
                            }
                        }
                        return false;
                    }
                    true
                });

                for child in childs.iter_mut() {
                    if child.get_processed() {
                        continue;
                    } else if let Some(log) = query_child(child) {
                        child.set_processed();
                        if let Some(vec) = vec {
                            vec.push(to_log(&log).into());
                            log_chaged = true;
                        }
                    };
                }

                if log_chaged {
                    state.set_logs(logs);
                }

                state.set_running_sc1(childs.len() as _);

                sleep(Duration::from_millis(100)).await;
            }
        }
    })?;

    app.show()?;
    tokio::task::block_in_place(run_event_loop)?;

    Ok(())
}
