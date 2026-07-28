use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use pinky_core::{
    create_vault, GocryptfsMount, OnboardedVault, SystemVaultPlatform, TaskManager, VaultPaths,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Serialize)]
struct RuntimeStatus {
    vault_mounted: bool,
    setup_in_progress: bool,
    vault_id: Option<Uuid>,
    prerequisites: BTreeMap<&'static str, bool>,
}

#[derive(Default)]
struct RuntimeData {
    setup_in_progress: bool,
    vault: Option<OnboardedVault<GocryptfsMount>>,
}

#[derive(Clone, Default)]
struct AppRuntime(Arc<Mutex<RuntimeData>>);

#[derive(Deserialize)]
struct SetupVaultRequest {
    cipher_dir: PathBuf,
    mount_dir: PathBuf,
    recovery_passphrase: String,
}

#[derive(Debug, Serialize)]
struct SetupVaultResponse {
    vault_id: Uuid,
    recovery_path: PathBuf,
}

#[tauri::command]
fn runtime_status(runtime: State<'_, AppRuntime>) -> RuntimeStatus {
    let data = runtime.0.lock().unwrap();
    let mounted = data
        .vault
        .as_ref()
        .is_some_and(|session| session.vault.ensure_mounted().is_ok());
    RuntimeStatus {
        vault_mounted: mounted,
        setup_in_progress: data.setup_in_progress,
        vault_id: data.vault.as_ref().map(|session| session.id),
        prerequisites: BTreeMap::from([
            ("gocryptfs", command_exists("gocryptfs")),
            ("podman", command_exists("podman")),
            ("secret_service", command_exists("secret-tool")),
            ("vulkan", command_exists("vulkaninfo")),
        ]),
    }
}

#[tauri::command]
fn default_vault_paths(app: AppHandle) -> Result<VaultPaths, String> {
    Ok(VaultPaths {
        cipher_dir: app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?
            .join("vault-cipher"),
        mount_dir: app
            .path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?
            .join("vault-mounted"),
    })
}

#[tauri::command]
async fn setup_vault(
    request: SetupVaultRequest,
    runtime: State<'_, AppRuntime>,
    tasks: State<'_, TaskManager>,
) -> Result<SetupVaultResponse, String> {
    let runtime = runtime.inner().clone();
    {
        let mut data = runtime.0.lock().unwrap();
        if data.vault.is_some() {
            return Err("the vault is already set up and mounted".into());
        }
        if data.setup_in_progress {
            return Err("vault setup is already in progress".into());
        }
        data.setup_in_progress = true;
    }

    let paths = VaultPaths {
        cipher_dir: request.cipher_dir,
        mount_dir: request.mount_dir,
    };
    let recovery_passphrase = Zeroizing::new(request.recovery_passphrase);
    let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
    tasks
        .inner()
        .clone()
        .spawn_uncancellable("vault setup", None, move |context| async move {
            context.progress(
                "vault setup",
                None,
                "Deriving recovery key and creating encrypted vault",
            );
            let outcome = tauri::async_runtime::spawn_blocking(move || {
                create_vault(&SystemVaultPlatform, paths, &recovery_passphrase)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| format!("vault setup worker failed: {error}"))
            .and_then(|result| result);
            let task_result = outcome
                .as_ref()
                .map(|_| ())
                .map_err(|message| message.clone());
            let _ = result_sender.send(outcome);
            task_result
        });
    let result = result_receiver
        .await
        .map_err(|_| "vault setup task ended without a result".to_owned());

    let mut data = runtime.0.lock().unwrap();
    data.setup_in_progress = false;
    match result? {
        Ok(vault) => {
            let response = SetupVaultResponse {
                vault_id: vault.id,
                recovery_path: vault.recovery_path.clone(),
            };
            data.vault = Some(vault);
            Ok(response)
        }
        Err(error) => Err(error.to_string()),
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
            app.manage(AppRuntime::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            runtime_status,
            default_vault_paths,
            setup_vault,
            start_system_check,
            cancel_task,
            pause_task,
            cancel_all_tasks
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Pinky desktop application");
}
