use std::{collections::BTreeMap, env, fs, time::Duration};

use pinky_core::TaskManager;
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

#[derive(Serialize)]
struct RuntimeStatus {
    vault_mounted: bool,
    prerequisites: BTreeMap<&'static str, bool>,
}

#[tauri::command]
fn runtime_status() -> RuntimeStatus {
    RuntimeStatus {
        // A vault is never inferred from a directory. It becomes available only
        // after the setup flow returns a verified pinky_core::Vault capability.
        vault_mounted: false,
        prerequisites: BTreeMap::from([
            ("gocryptfs", command_exists("gocryptfs")),
            ("podman", command_exists("podman")),
            ("secret_service", command_exists("secret-tool")),
            ("vulkan", command_exists("vulkaninfo")),
        ]),
    }
}

#[tauri::command]
fn start_system_check(tasks: State<'_, TaskManager>) -> String {
    tasks
        .spawn("system check", None, |mut context| async move {
            let checks = [
                "gocryptfs",
                "Secret Service",
                "rootless Podman",
                "Vulkan",
                "disk and memory",
            ];
            for (index, check) in checks.iter().enumerate() {
                context
                    .checkpoint()
                    .await
                    .map_err(|_| "cancelled".to_owned())?;
                context.progress(
                    "system check",
                    Some(index as f32 / checks.len() as f32),
                    format!("Checking {check}"),
                );
                tokio::time::sleep(Duration::from_millis(450)).await;
            }
            Ok(())
        })
        .to_string()
}

#[tauri::command]
fn cancel_task(task_id: String, tasks: State<'_, TaskManager>) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|_| "invalid task UUID".to_owned())?;
    if tasks.cancel(id) {
        let manager = tasks.inner().clone();
        tauri::async_runtime::spawn(async move {
            manager.enforce_cancel_deadlines(id).await;
        });
        Ok(())
    } else {
        Err("task not found".into())
    }
}

#[tauri::command]
fn pause_task(task_id: String, paused: bool, tasks: State<'_, TaskManager>) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|_| "invalid task UUID".to_owned())?;
    tasks
        .pause(id, paused)
        .then_some(())
        .ok_or_else(|| "task not found".into())
}

#[tauri::command]
fn cancel_all_tasks(tasks: State<'_, TaskManager>) -> usize {
    tasks.cancel_all()
}

fn command_exists(program: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths).any(|directory| {
                fs::metadata(directory.join(program))
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let tasks = TaskManager::new();
            let mut events = tasks.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = events.recv().await {
                    let _ = handle.emit("pinky://task-event", event);
                }
            });
            app.manage(tasks);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            runtime_status,
            start_system_check,
            cancel_task,
            pause_task,
            cancel_all_tasks
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Pinky desktop application");
}
