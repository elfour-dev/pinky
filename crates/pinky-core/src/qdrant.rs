use std::{
    fmt, fs,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    time::Duration,
};

use rand::RngCore;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::{process::Command, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{ProcessError, ProcessOutcome, ProcessSupervisor, Vault, VaultError};

const COLLECTION: &str = "pinky_chunks_v1";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Error)]
pub enum QdrantError {
    #[error("vault unavailable: {0}")]
    Vault(#[from] VaultError),
    #[error("Qdrant executable must be an absolute regular file: {0}")]
    InvalidExecutable(PathBuf),
    #[error("Qdrant I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Qdrant request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Qdrant rejected a request with HTTP {status}: {body}")]
    Response { status: StatusCode, body: String },
    #[error("Qdrant collection has vector size {found}, expected {expected}")]
    DimensionMismatch { found: usize, expected: usize },
    #[error("Qdrant vector dimension must be greater than zero")]
    InvalidDimension,
    #[error("Qdrant vector for point {point_id} has {found} dimensions, expected {expected}")]
    InvalidVector {
        point_id: Uuid,
        found: usize,
        expected: usize,
    },
    #[error("Qdrant sidecar did not become ready within 15 seconds")]
    StartupTimeout,
    #[error("Qdrant sidecar exited during startup: {0:?}")]
    Exited(ProcessOutcome),
    #[error("Qdrant supervisor failed: {0}")]
    Process(#[from] ProcessError),
    #[error("Qdrant supervisor task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub struct QdrantLaunchConfig {
    executable: PathBuf,
    storage_path: PathBuf,
    snapshots_path: PathBuf,
    http_port: u16,
    grpc_port: u16,
    api_key: Zeroizing<String>,
}

impl fmt::Debug for QdrantLaunchConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QdrantLaunchConfig")
            .field("executable", &self.executable)
            .field("storage_path", &self.storage_path)
            .field("snapshots_path", &self.snapshots_path)
            .field("http_port", &self.http_port)
            .field("grpc_port", &self.grpc_port)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl QdrantLaunchConfig {
    pub fn new(executable: impl AsRef<Path>, vault: &Vault) -> Result<Self, QdrantError> {
        let http_port = reserve_loopback_port()?;
        let mut grpc_port = reserve_loopback_port()?;
        while grpc_port == http_port {
            grpc_port = reserve_loopback_port()?;
        }
        Self::new_with_ports(executable, vault, http_port, grpc_port)
    }

    fn new_with_ports(
        executable: impl AsRef<Path>,
        vault: &Vault,
        http_port: u16,
        grpc_port: u16,
    ) -> Result<Self, QdrantError> {
        vault.ensure_mounted()?;
        let executable = executable.as_ref();
        if !executable.is_absolute()
            || !fs::metadata(executable).is_ok_and(|metadata| metadata.is_file())
        {
            return Err(QdrantError::InvalidExecutable(executable.to_owned()));
        }
        let executable = fs::canonicalize(executable)?;
        let storage_path = vault.resolve_internal("indexes/qdrant");
        let snapshots_path = vault.resolve_internal("snapshots/qdrant");
        secure_directory(vault, &storage_path)?;
        secure_directory(vault, &snapshots_path)?;
        let mut token = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut token);
        Ok(Self {
            executable,
            storage_path,
            snapshots_path,
            http_port,
            grpc_port,
            api_key: Zeroizing::new(hex::encode(token)),
        })
    }

    pub fn endpoint(&self) -> Result<QdrantClient, QdrantError> {
        QdrantClient::loopback(self.http_port, self.api_key.clone())
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .env("RUN_MODE", "production")
            .env("QDRANT__TELEMETRY_DISABLED", "true")
            .env("QDRANT__SERVICE__HOST", "127.0.0.1")
            .env("QDRANT__SERVICE__HTTP_PORT", self.http_port.to_string())
            .env("QDRANT__SERVICE__GRPC_PORT", self.grpc_port.to_string())
            .env("QDRANT__SERVICE__ENABLE_CORS", "false")
            .env("QDRANT__SERVICE__API_KEY", self.api_key.as_str())
            .env("QDRANT__STORAGE__STORAGE_PATH", &self.storage_path)
            .env("QDRANT__STORAGE__SNAPSHOTS_PATH", &self.snapshots_path);
        command
    }
}

#[derive(Clone)]
pub struct QdrantClient {
    client: Client,
    base_url: String,
    api_key: Zeroizing<String>,
}

impl fmt::Debug for QdrantClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QdrantClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorPoint {
    pub id: Uuid,
    pub source_version_id: Uuid,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorMatch {
    pub id: Uuid,
    pub source_version_id: Uuid,
    pub score: f32,
}

impl QdrantClient {
    fn loopback(port: u16, api_key: Zeroizing<String>) -> Result<Self, QdrantError> {
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(30))
                .build()?,
            base_url: format!("http://127.0.0.1:{port}"),
            api_key,
        })
    }

    pub async fn health(&self) -> Result<(), QdrantError> {
        let response = self
            .auth(self.client.get(format!("{}/healthz", self.base_url)))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(QdrantError::Response {
                status: response.status(),
                body: response.text().await?,
            })
        }
    }

    pub async fn ensure_collection(&self, dimension: usize) -> Result<(), QdrantError> {
        if dimension == 0 {
            return Err(QdrantError::InvalidDimension);
        }
        let url = format!("{}/collections/{COLLECTION}", self.base_url);
        let response = self.auth(self.client.get(&url)).send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            self.request(self.client.put(url).json(&json!({
                "vectors": { "size": dimension, "distance": "Cosine", "on_disk": true },
                "hnsw_config": { "on_disk": true },
                "quantization_config": { "scalar": { "type": "int8", "quantile": 0.99, "always_ram": false } },
                "on_disk_payload": true,
                "metadata": { "schema_version": 1 }
            }))).await?;
            return Ok(());
        }
        let value = parse_response(response).await?;
        let found = value
            .pointer("/result/config/params/vectors/size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default() as usize;
        if found != dimension {
            return Err(QdrantError::DimensionMismatch {
                found,
                expected: dimension,
            });
        }
        Ok(())
    }

    pub async fn upsert(
        &self,
        dimension: usize,
        points: &[VectorPoint],
    ) -> Result<(), QdrantError> {
        for point in points {
            if point.vector.len() != dimension {
                return Err(QdrantError::InvalidVector {
                    point_id: point.id,
                    found: point.vector.len(),
                    expected: dimension,
                });
            }
        }
        let points = points
            .iter()
            .map(|point| {
                json!({
                    "id": point.id,
                    "vector": normalize(&point.vector),
                    "payload": { "source_version_id": point.source_version_id }
                })
            })
            .collect::<Vec<_>>();
        self.request(
            self.client
                .put(format!(
                    "{}/collections/{COLLECTION}/points?wait=true",
                    self.base_url
                ))
                .json(&json!({ "points": points })),
        )
        .await?;
        Ok(())
    }

    pub async fn query(
        &self,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorMatch>, QdrantError> {
        if vector.is_empty() {
            return Err(QdrantError::InvalidDimension);
        }
        let value = self.request(self.client.post(format!("{}/collections/{COLLECTION}/points/query", self.base_url)).json(&json!({
            "query": normalize(vector), "limit": limit, "with_payload": true, "with_vector": false
        }))).await?;
        value
            .pointer("/result/points")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|point| {
                Ok(VectorMatch {
                    id: point
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|id| Uuid::parse_str(id).ok())
                        .ok_or_else(|| QdrantError::Response {
                            status: StatusCode::OK,
                            body: "point response has invalid id".into(),
                        })?,
                    source_version_id: point
                        .pointer("/payload/source_version_id")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|id| Uuid::parse_str(id).ok())
                        .ok_or_else(|| QdrantError::Response {
                            status: StatusCode::OK,
                            body: "point response has invalid source version".into(),
                        })?,
                    score: point
                        .get("score")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or_default() as f32,
                })
            })
            .collect()
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("api-key", self.api_key.as_str())
    }

    async fn request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<serde_json::Value, QdrantError> {
        parse_response(self.auth(request).send().await?).await
    }
}

pub struct QdrantSidecar {
    pub client: QdrantClient,
    cancellation: CancellationToken,
    supervisor: Option<JoinHandle<Result<ProcessOutcome, ProcessError>>>,
}

impl QdrantSidecar {
    pub async fn start(config: QdrantLaunchConfig) -> Result<Self, QdrantError> {
        let client = config.endpoint()?;
        let cancellation = CancellationToken::new();
        let process_cancellation = cancellation.clone();
        let command = config.command();
        let mut supervisor = tokio::spawn(async move {
            ProcessSupervisor::new()
                .run(command, process_cancellation)
                .await
        });
        let readiness = tokio::time::timeout(STARTUP_TIMEOUT, async {
            loop {
                if supervisor.is_finished() {
                    let outcome = (&mut supervisor).await??;
                    return Err(QdrantError::Exited(outcome));
                }
                if client.health().await.is_ok() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        match readiness {
            Ok(Ok(())) => Ok(Self {
                client,
                cancellation,
                supervisor: Some(supervisor),
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                cancellation.cancel();
                let _ = supervisor.await;
                Err(QdrantError::StartupTimeout)
            }
        }
    }

    pub async fn shutdown(mut self) -> Result<ProcessOutcome, QdrantError> {
        self.cancellation.cancel();
        Ok(self
            .supervisor
            .take()
            .expect("running sidecar has a supervisor")
            .await??)
    }
}

impl Drop for QdrantSidecar {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(supervisor) = self.supervisor.take() {
            supervisor.abort();
        }
    }
}

async fn parse_response(response: reqwest::Response) -> Result<serde_json::Value, QdrantError> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(QdrantError::Response { status, body });
    }
    serde_json::from_str(&body).map_err(|error| QdrantError::Response {
        status,
        body: error.to_string(),
    })
}

fn normalize(vector: &[f32]) -> Vec<f32> {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if magnitude == 0.0 {
        return vector.to_vec();
    }
    vector.iter().map(|value| value / magnitude).collect()
}

fn reserve_loopback_port() -> Result<u16, std::io::Error> {
    Ok(
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port(),
    )
}

fn secure_directory(vault: &Vault, path: &Path) -> Result<(), QdrantError> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(QdrantError::Vault(VaultError::UnsafeLayout(
            path.to_owned(),
        )));
    }
    fs::create_dir_all(path)?;
    if !fs::canonicalize(path)?.starts_with(vault.root()) {
        return Err(QdrantError::Vault(VaultError::UnsafeLayout(
            path.to_owned(),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MountVerifier;

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[test]
    fn launch_configuration_keeps_storage_inside_the_vault_and_redacts_the_key() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let executable = root.path().join("qdrant");
        fs::write(&executable, b"test executable").unwrap();
        let config = QdrantLaunchConfig::new_with_ports(&executable, &vault, 41001, 41002).unwrap();
        assert!(config.storage_path.starts_with(vault.root()));
        assert!(config.snapshots_path.starts_with(vault.root()));
        assert!(!format!("{config:?}").contains(config.api_key.as_str()));
        let command = config.command();
        let environments = command.as_std().get_envs().collect::<Vec<_>>();
        assert!(environments
            .iter()
            .any(|(key, value)| *key == "QDRANT__SERVICE__HOST"
                && value.is_some_and(|value| value == "127.0.0.1")));
    }

    #[test]
    fn refuses_qdrant_storage_symlinked_outside_the_vault() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        symlink(outside.path(), vault.root().join("indexes/qdrant")).unwrap();
        let executable = root.path().join("qdrant");
        fs::write(&executable, b"test executable").unwrap();
        assert!(matches!(
            QdrantLaunchConfig::new_with_ports(&executable, &vault, 41001, 41002),
            Err(QdrantError::Vault(VaultError::UnsafeLayout(_)))
        ));
    }

    #[test]
    fn vectors_are_normalized_before_storage_and_search() {
        assert_eq!(normalize(&[3.0, 4.0]), vec![0.6, 0.8]);
        assert_eq!(normalize(&[0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn every_sidecar_request_carries_the_launch_key() {
        let key = Zeroizing::new("a".repeat(64));
        let client = QdrantClient::loopback(41001, key.clone()).unwrap();
        let request = client
            .auth(client.client.get(format!("{}/healthz", client.base_url)))
            .build()
            .unwrap();
        assert_eq!(request.headers().get("api-key").unwrap(), key.as_str());
        assert_eq!(request.url().host_str(), Some("127.0.0.1"));
    }
}
