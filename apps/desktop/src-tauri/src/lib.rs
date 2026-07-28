use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use pinky_core::{
    create_registered_vault, read_registration, unlock_registered_vault, GocryptfsMount,
    ObjectStore, OnboardedVault, SystemVaultPlatform, TaskJournal, TaskManager, VaultPaths,
    VaultRegistration,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Serialize)]
struct RuntimeStatus {
    vault_mounted: bool,
    setup_in_progress: bool,
    vault_registered: bool,
    vault_id: Option<Uuid>,
    unlock_error: Option<String>,
    task_journal_error: Option<String>,
    prerequisites: BTreeMap<&'static str, bool>,
}

#[derive(Default)]
struct RuntimeData {
    setup_in_progress: bool,
    registration: Option<VaultRegistration>,
    unlock_error: Option<String>,
    vault: Option<OnboardedVault<GocryptfsMount>>,
}

#[derive(Clone)]
struct AppRuntime {
    data: Arc<Mutex<RuntimeData>>,
    registration_path: Arc<PathBuf>,
}

impl AppRuntime {
    fn new(
        registration_path: PathBuf,
        registration: Option<VaultRegistration>,
        error: Option<String>,
    ) -> Self {
        Self {
            data: Arc::new(Mutex::new(RuntimeData {
                setup_in_progress: false,
                registration,
                unlock_error: error,
                vault: None,
            })),
            registration_path: Arc::new(registration_path),
        }
    }
}

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
fn runtime_status(runtime: State<'_, AppRuntime>, tasks: State<'_, TaskManager>) -> RuntimeStatus {
    let data = runtime.data.lock().unwrap();
    let mounted = data
        .vault
        .as_ref()
        .is_some_and(|session| session.vault.ensure_mounted().is_ok());
    RuntimeStatus {
        vault_mounted: mounted,
        setup_in_progress: data.setup_in_progress,
        vault_registered: data.registration.is_some() || runtime.registration_path.exists(),
        vault_id: data.vault.as_ref().map(|session| session.id).or_else(|| {
            data.registration
                .as_ref()
                .map(|registration| registration.vault_id)
        }),
        unlock_error: data.unlock_error.clone(),
        task_journal_error: tasks.journal_error(),
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
        let mut data = runtime.data.lock().unwrap();
        if data.vault.is_some() {
            return Err("the vault is already set up and mounted".into());
        }
        if data.registration.is_some() || runtime.registration_path.exists() {
            return Err("a vault is already registered; unlock or repair it instead".into());
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
    let registration_paths = paths.clone();
    let registration_path = runtime.registration_path.as_ref().clone();
    let recovery_passphrase = Zeroizing::new(request.recovery_passphrase);
    let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
    let task_manager = tasks.inner().clone();
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
                let vault = create_registered_vault(
                    &SystemVaultPlatform,
                    paths,
                    &recovery_passphrase,
                    &registration_path,
                )
                .map_err(|error| error.to_string())?;
                let journal = TaskJournal::new(
                    vault.database.clone(),
                    ObjectStore::new(vault.vault.clone()),
                );
                task_manager
                    .attach_journal(journal)
                    .map_err(|error| error.to_string())?;
                let registration = vault.registration(registration_paths);
                Ok((vault, registration))
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

    let mut data = runtime.data.lock().unwrap();
    data.setup_in_progress = false;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            data.unlock_error = Some(error.clone());
            return Err(error);
        }
    };
    match result {
        Ok((vault, registration)) => {
            let response = SetupVaultResponse {
                vault_id: vault.id,
                recovery_path: vault.recovery_path.clone(),
            };
            data.registration = Some(registration);
            data.unlock_error = None;
            data.vault = Some(vault);
            Ok(response)
        }
        Err(error) => {
            data.unlock_error = Some(error.clone());
            Err(error)
        }
    }
}

#[tauri::command]
async fn unlock_vault(
    runtime: State<'_, AppRuntime>,
    tasks: State<'_, TaskManager>,
) -> Result<(), String> {
    unlock_runtime(runtime.inner().clone(), tasks.inner().clone()).await
}

async fn unlock_runtime(runtime: AppRuntime, tasks: TaskManager) -> Result<(), String> {
    let registration = {
        let mut data = runtime.data.lock().unwrap();
        if data
            .vault
            .as_ref()
            .is_some_and(|session| session.vault.ensure_mounted().is_ok())
        {
            return Ok(());
        }
        if data.setup_in_progress {
            return Err("a vault operation is already in progress".into());
        }
        let registration = data
            .registration
            .clone()
            .ok_or_else(|| "no valid vault registration was found".to_owned())?;
        data.setup_in_progress = true;
        data.unlock_error = None;
        registration
    };

    let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
    let task_manager = tasks.clone();
    tasks.spawn_uncancellable("vault unlock", None, move |context| async move {
        context.progress(
            "vault unlock",
            None,
            "Retrieving the vault key from Secret Service",
        );
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            let vault = unlock_registered_vault(&SystemVaultPlatform, &registration)
                .map_err(|error| error.to_string())?;
            let journal = TaskJournal::new(
                vault.database.clone(),
                ObjectStore::new(vault.vault.clone()),
            );
            task_manager
                .attach_journal(journal)
                .map_err(|error| error.to_string())?;
            Ok(vault)
        })
        .await
        .map_err(|error| format!("vault unlock worker failed: {error}"))
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
        .map_err(|_| "vault unlock task ended without a result".to_owned());
    let mut data = runtime.data.lock().unwrap();
    data.setup_in_progress = false;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            data.unlock_error = Some(error.clone());
            return Err(error);
        }
    };
    match result {
        Ok(vault) => {
            data.vault = Some(vault);
            data.unlock_error = None;
            Ok(())
        }
        Err(error) => {
            data.unlock_error = Some(error.clone());
            Err(error)
        }
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

#[tauri::command]
fn task_snapshot(tasks: State<'_, TaskManager>) -> Vec<pinky_core::TaskEvent> {
    tasks.snapshot()
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
            let registration_path = app.path().app_config_dir()?.join("vault-registration.json");
            let (registration, registration_error) = if registration_path.exists() {
                match read_registration(&registration_path) {
                    Ok(registration) => (Some(registration), None),
                    Err(error) => (None, Some(error.to_string())),
                }
            } else {
                (None, None)
            };
            let runtime = AppRuntime::new(registration_path, registration, registration_error);
            let mut events = tasks.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = events.recv().await {
                    let _ = handle.emit("pinky://task-event", event);
                }
            });
            app.manage(tasks.clone());
            app.manage(runtime.clone());
            let should_unlock = runtime.data.lock().unwrap().registration.is_some();
            if should_unlock {
                tauri::async_runtime::spawn(async move {
                    let _ = unlock_runtime(runtime, tasks).await;
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            runtime_status,
            default_vault_paths,
            setup_vault,
            unlock_vault,
            start_system_check,
            cancel_task,
            pause_task,
            cancel_all_tasks,
            task_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Pinky desktop application");
}
