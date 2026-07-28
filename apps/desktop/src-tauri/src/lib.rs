use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use pinky_core::{
    create_registered_vault, read_registration, unlock_registered_vault, CitationPassage,
    GocryptfsMount, LocalFileFingerprint, LocalIngestor, LocalWatchTarget, ObjectStore,
    OnboardedVault, RetrievalService, SearchHit, SourceSummary, SystemVaultPlatform, TaskJournal,
    TaskManager, VaultPaths, VaultRegistration,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
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
    watcher_error: Option<String>,
    prerequisites: BTreeMap<&'static str, bool>,
}

#[derive(Default)]
struct RuntimeData {
    setup_in_progress: bool,
    registration: Option<VaultRegistration>,
    unlock_error: Option<String>,
    watcher_error: Option<String>,
    vault: Option<OnboardedVault<GocryptfsMount>>,
}

#[derive(Clone)]
struct AppRuntime {
    data: Arc<Mutex<RuntimeData>>,
    registration_path: Arc<PathBuf>,
    watcher: Arc<Mutex<Option<CancellationToken>>>,
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
                watcher_error: None,
                vault: None,
            })),
            registration_path: Arc::new(registration_path),
            watcher: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Deserialize)]
struct SetupVaultRequest {
    cipher_dir: PathBuf,
    mount_dir: PathBuf,
    recovery_passphrase: String,
}

#[derive(Deserialize)]
struct IngestLocalFileRequest {
    approved_root: PathBuf,
    source_path: PathBuf,
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
        watcher_error: data.watcher_error.clone(),
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
            drop(data);
            start_local_watcher(runtime.clone(), tasks.inner().clone());
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
            drop(data);
            start_local_watcher(runtime.clone(), tasks.clone());
            Ok(())
        }
        Err(error) => {
            data.unlock_error = Some(error.clone());
            Err(error)
        }
    }
}

#[tauri::command]
async fn start_system_check(tasks: State<'_, TaskManager>) -> Result<String, String> {
    Ok(tasks
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
        .to_string())
}

fn local_ingestor(runtime: &AppRuntime) -> Result<LocalIngestor, String> {
    let data = runtime
        .data
        .lock()
        .map_err(|_| "runtime lock is poisoned".to_owned())?;
    let vault = data
        .vault
        .as_ref()
        .ok_or_else(|| "the encrypted vault is not unlocked".to_owned())?;
    vault
        .vault
        .ensure_mounted()
        .map_err(|error| error.to_string())?;
    Ok(LocalIngestor::new(
        vault.database.clone(),
        ObjectStore::new(vault.vault.clone()),
    ))
}

fn retrieval_service(runtime: &AppRuntime) -> Result<RetrievalService, String> {
    let data = runtime
        .data
        .lock()
        .map_err(|_| "runtime lock is poisoned".to_owned())?;
    let vault = data
        .vault
        .as_ref()
        .ok_or_else(|| "the encrypted vault is not unlocked".to_owned())?;
    vault
        .vault
        .ensure_mounted()
        .map_err(|error| error.to_string())?;
    Ok(RetrievalService::new(
        vault.database.clone(),
        ObjectStore::new(vault.vault.clone()),
    ))
}

struct PendingObservation {
    fingerprint: LocalFileFingerprint,
    first_seen: Instant,
    confirmations: u8,
}

struct WatchCompletion {
    source_id: Uuid,
    succeeded: bool,
    sender: tokio::sync::mpsc::UnboundedSender<(Uuid, bool)>,
}

impl Drop for WatchCompletion {
    fn drop(&mut self) {
        let _ = self.sender.send((self.source_id, self.succeeded));
    }
}

fn stable_change_ready(
    pending: &mut HashMap<Uuid, PendingObservation>,
    source_id: Uuid,
    fingerprint: LocalFileFingerprint,
    now: Instant,
) -> bool {
    let observation = pending
        .entry(source_id)
        .or_insert_with(|| PendingObservation {
            fingerprint: fingerprint.clone(),
            first_seen: now,
            confirmations: 0,
        });
    if observation.fingerprint != fingerprint {
        *observation = PendingObservation {
            fingerprint,
            first_seen: now,
            confirmations: 0,
        };
    }
    observation.confirmations = observation.confirmations.saturating_add(1);
    observation.confirmations >= 2
        && now.duration_since(observation.first_seen) >= Duration::from_millis(750)
}

fn start_local_watcher(runtime: AppRuntime, tasks: TaskManager) {
    let mut watcher = runtime.watcher.lock().unwrap();
    if watcher.is_some() {
        return;
    }
    let ingestor = match local_ingestor(&runtime) {
        Ok(ingestor) => ingestor,
        Err(error) => {
            runtime.data.lock().unwrap().watcher_error = Some(error);
            return;
        }
    };
    let cancellation = CancellationToken::new();
    *watcher = Some(cancellation.clone());
    drop(watcher);
    tauri::async_runtime::spawn(run_local_watcher(runtime, tasks, ingestor, cancellation));
}

async fn run_local_watcher(
    runtime: AppRuntime,
    tasks: TaskManager,
    ingestor: LocalIngestor,
    cancellation: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pending = HashMap::<Uuid, PendingObservation>::new();
    let mut in_flight = HashSet::<Uuid>::new();
    let mut cooldown = HashMap::<Uuid, Instant>::new();
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<(Uuid, bool)>();

    loop {
        tokio::select! {
            _ = cancellation.cancelled() => break,
            _ = interval.tick() => {}
        }
        while let Ok((source_id, succeeded)) = done_rx.try_recv() {
            in_flight.remove(&source_id);
            if succeeded {
                cooldown.remove(&source_id);
            } else {
                cooldown.insert(source_id, Instant::now());
            }
        }

        let target_ingestor = ingestor.clone();
        let targets =
            match tauri::async_runtime::spawn_blocking(move || target_ingestor.watch_targets())
                .await
            {
                Ok(Ok(targets)) => targets,
                Ok(Err(error)) => {
                    runtime.data.lock().unwrap().watcher_error = Some(error.to_string());
                    continue;
                }
                Err(error) => {
                    runtime.data.lock().unwrap().watcher_error =
                        Some(format!("local watcher worker failed: {error}"));
                    continue;
                }
            };
        runtime.data.lock().unwrap().watcher_error = None;

        let known = targets
            .iter()
            .map(|target| target.source_id)
            .collect::<HashSet<_>>();
        pending.retain(|source_id, _| known.contains(source_id));
        cooldown.retain(|source_id, _| known.contains(source_id));

        for target in targets {
            if in_flight.contains(&target.source_id)
                || cooldown
                    .get(&target.source_id)
                    .is_some_and(|last| last.elapsed() < Duration::from_secs(30))
            {
                continue;
            }
            match LocalFileFingerprint::read(&target.source_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    pending.remove(&target.source_id);
                    if target.state != "missing" {
                        in_flight.insert(target.source_id);
                        spawn_missing_source_task(
                            &tasks,
                            ingestor.clone(),
                            target.source_id,
                            done_tx.clone(),
                        );
                    }
                }
                Err(error) => {
                    runtime.data.lock().unwrap().watcher_error = Some(format!(
                        "cannot observe {}: {error}",
                        target.source_path.display()
                    ));
                }
                Ok(fingerprint)
                    if target.state == "missing"
                        || target.fingerprint.as_ref() != Some(&fingerprint) =>
                {
                    let now = Instant::now();
                    if stable_change_ready(&mut pending, target.source_id, fingerprint, now) {
                        pending.remove(&target.source_id);
                        in_flight.insert(target.source_id);
                        spawn_watched_ingestion_task(
                            &tasks,
                            ingestor.clone(),
                            target,
                            done_tx.clone(),
                        );
                    }
                }
                Ok(_) => {
                    pending.remove(&target.source_id);
                    cooldown.remove(&target.source_id);
                }
            }
        }
    }
}

fn spawn_watched_ingestion_task(
    tasks: &TaskManager,
    ingestor: LocalIngestor,
    target: LocalWatchTarget,
    done: tokio::sync::mpsc::UnboundedSender<(Uuid, bool)>,
) {
    tasks.spawn("local ingestion", None, move |mut context| async move {
        let mut completion = WatchCompletion {
            source_id: target.source_id,
            succeeded: false,
            sender: done,
        };
        context
            .checkpoint()
            .await
            .map_err(|_| "cancelled".to_owned())?;
        context.progress(
            "local ingestion",
            Some(0.1),
            format!(
                "Refreshing {} after a stable change",
                target.source_path.display()
            ),
        );
        let cancellation = context.cancellation_token();
        let result = tauri::async_runtime::spawn_blocking(move || {
            ingestor.ingest_cancellable(target.approved_root, target.source_path, cancellation)
        })
        .await
        .map_err(|error| format!("ingestion worker failed: {error}"))?
        .map_err(|error| error.to_string());
        completion.succeeded = result.is_ok();
        let result = result?;
        context.progress(
            "local ingestion",
            Some(0.95),
            format!(
                "Retained refreshed version in {} chunks",
                result.chunk_count
            ),
        );
        Ok(())
    });
}

fn spawn_missing_source_task(
    tasks: &TaskManager,
    ingestor: LocalIngestor,
    source_id: Uuid,
    done: tokio::sync::mpsc::UnboundedSender<(Uuid, bool)>,
) {
    tasks.spawn("local ingestion", None, move |mut context| async move {
        let mut completion = WatchCompletion {
            source_id,
            succeeded: false,
            sender: done,
        };
        context
            .checkpoint()
            .await
            .map_err(|_| "cancelled".to_owned())?;
        context.progress(
            "local ingestion",
            Some(0.5),
            "Marking a deleted local source as missing",
        );
        let result = tauri::async_runtime::spawn_blocking(move || ingestor.mark_missing(source_id))
            .await
            .map_err(|error| format!("local watcher worker failed: {error}"))?
            .map_err(|error| error.to_string());
        completion.succeeded = result.is_ok();
        result.map(|_| ())
    });
}

#[tauri::command]
async fn ingest_local_file(
    request: IngestLocalFileRequest,
    runtime: State<'_, AppRuntime>,
    tasks: State<'_, TaskManager>,
) -> Result<String, String> {
    let ingestor = local_ingestor(runtime.inner())?;
    let task_id = tasks.spawn("local ingestion", None, move |mut context| async move {
        context
            .checkpoint()
            .await
            .map_err(|_| "cancelled".to_owned())?;
        context.progress(
            "local ingestion",
            Some(0.1),
            "Validating approved path and archiving source",
        );
        let cancellation = context.cancellation_token();
        let result = tauri::async_runtime::spawn_blocking(move || {
            ingestor.ingest_cancellable(request.approved_root, request.source_path, cancellation)
        })
        .await
        .map_err(|error| format!("ingestion worker failed: {error}"))?
        .map_err(|error| error.to_string())?;
        context
            .checkpoint()
            .await
            .map_err(|_| "cancelled".to_owned())?;
        context.progress(
            "local ingestion",
            Some(0.95),
            format!(
                "Retained {} bytes in {} chunk{}",
                result.byte_size,
                result.chunk_count,
                if result.chunk_count == 1 { "" } else { "s" }
            ),
        );
        Ok(())
    });
    Ok(task_id.to_string())
}

#[tauri::command]
fn list_sources(runtime: State<'_, AppRuntime>) -> Result<Vec<SourceSummary>, String> {
    local_ingestor(runtime.inner())?
        .list_sources()
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn search_sources(
    query: String,
    limit: Option<usize>,
    runtime: State<'_, AppRuntime>,
    tasks: State<'_, TaskManager>,
) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Err("enter a search query".to_owned());
    }
    let retrieval = retrieval_service(runtime.inner())?;
    let limit = limit.unwrap_or(8).clamp(1, 12);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tasks.spawn("lexical retrieval", None, move |mut context| async move {
        context
            .checkpoint()
            .await
            .map_err(|_| "cancelled".to_owned())?;
        context.progress(
            "lexical retrieval",
            Some(0.15),
            "Searching retained source passages",
        );
        let outcome = tauri::async_runtime::spawn_blocking(move || retrieval.search(&query, limit))
            .await
            .map_err(|error| format!("retrieval worker failed: {error}"))
            .and_then(|result| result.map_err(|error| error.to_string()));
        let task_result = outcome.as_ref().map(|_| ()).map_err(Clone::clone);
        let _ = sender.send(outcome);
        task_result
    });
    receiver
        .await
        .map_err(|_| "retrieval task ended without a result".to_owned())?
}

#[tauri::command]
fn open_citation(
    citation_uri: String,
    runtime: State<'_, AppRuntime>,
) -> Result<CitationPassage, String> {
    retrieval_service(runtime.inner())?
        .open_citation(&citation_uri)
        .map_err(|error| error.to_string())
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

#[cfg(test)]
mod watcher_tests {
    use super::*;
    use pinky_core::{Database, MountVerifier, Vault};
    use std::path::Path;

    fn fingerprint(byte_size: u64, modified_nanoseconds: i64) -> LocalFileFingerprint {
        LocalFileFingerprint {
            device: 1,
            inode: 2,
            byte_size,
            modified_seconds: 100,
            modified_nanoseconds,
        }
    }

    #[test]
    fn waits_for_stable_checks_and_the_debounce_window() {
        let id = Uuid::new_v4();
        let started = Instant::now();
        let mut pending = HashMap::new();
        assert!(!stable_change_ready(
            &mut pending,
            id,
            fingerprint(10, 1),
            started,
        ));
        assert!(!stable_change_ready(
            &mut pending,
            id,
            fingerprint(10, 1),
            started + Duration::from_millis(500),
        ));
        assert!(stable_change_ready(
            &mut pending,
            id,
            fingerprint(10, 1),
            started + Duration::from_millis(1_000),
        ));
    }

    #[test]
    fn changed_fingerprint_restarts_the_stability_window() {
        let id = Uuid::new_v4();
        let started = Instant::now();
        let mut pending = HashMap::new();
        assert!(!stable_change_ready(
            &mut pending,
            id,
            fingerprint(10, 1),
            started,
        ));
        assert!(!stable_change_ready(
            &mut pending,
            id,
            fingerprint(11, 2),
            started + Duration::from_millis(800),
        ));
        assert!(!stable_change_ready(
            &mut pending,
            id,
            fingerprint(11, 2),
            started + Duration::from_millis(1_300),
        ));
        assert!(stable_change_ready(
            &mut pending,
            id,
            fingerprint(11, 2),
            started + Duration::from_millis(1_800),
        ));
    }

    struct Mounted;

    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_reversions_stable_edits_and_marks_deletions_missing() {
        let vault_root = tempfile::tempdir().unwrap();
        let approved_root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x37; 32])).unwrap(),
        ));
        let ingestor = LocalIngestor::new(database, ObjectStore::new(vault));
        let source = approved_root.path().join("watched.txt");
        fs::write(&source, "initial watched content").unwrap();
        let first = ingestor.ingest(approved_root.path(), &source).unwrap();

        let runtime = AppRuntime::new(vault_root.path().join("registration.json"), None, None);
        let tasks = TaskManager::new();
        let cancellation = CancellationToken::new();
        let watcher = tokio::spawn(run_local_watcher(
            runtime,
            tasks,
            ingestor.clone(),
            cancellation.clone(),
        ));

        fs::write(&source, "stable replacement watched content").unwrap();
        let second = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let summary = ingestor.list_sources().unwrap().remove(0);
                if summary.version_id != first.version_id {
                    break summary;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(second.state, "active");

        fs::remove_file(&source).unwrap();
        let missing = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let summary = ingestor.list_sources().unwrap().remove(0);
                if summary.state == "missing" {
                    break summary;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(missing.version_id, second.version_id);

        cancellation.cancel();
        watcher.await.unwrap();
    }
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
            ingest_local_file,
            list_sources,
            search_sources,
            open_citation,
            cancel_task,
            pause_task,
            cancel_all_tasks,
            task_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Pinky desktop application");
}
