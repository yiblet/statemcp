//! Short-lived HTTP capabilities and streaming multipart/archive transport.
use crate::{BlockingError, BlockingExecutor};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, FromRequest, Multipart, Query, Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use state_contract::{execution::PublicConfig, transfer::*};
use state_core::{DownloadSelection, Error, ErrorCode};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Semaphore, mpsc},
};
use tokio_stream::wrappers::ReceiverStream;

const TOKEN_LIFETIME: Duration = Duration::from_secs(600);
const MAX_TOKENS: usize = 1024;
const MULTIPART_OVERHEAD: usize = 256 * 1024;

enum Grant {
    Upload {
        namespace: String,
        paths: UploadPaths,
    },
    Download(DownloadSelection),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Upload,
    Download,
}
impl Grant {
    fn direction(&self) -> Direction {
        match self {
            Self::Upload { .. } => Direction::Upload,
            Self::Download(_) => Direction::Download,
        }
    }
    fn paths(&self) -> &[TransferPath] {
        match self {
            Self::Upload { paths, .. } => paths.paths(),
            Self::Download(selection) => &selection.paths,
        }
    }
}
struct Pending {
    expires: Instant,
    grant: Grant,
}
/// Shared token state for all HTTP MCP sessions and listeners; absent in stdio.
#[derive(Clone)]
pub struct Transfers {
    executor: BlockingExecutor,
    config: PublicConfig,
    pending: Arc<Mutex<BTreeMap<String, Pending>>>,
    uploads: Arc<Semaphore>,
}
impl Transfers {
    /// Create an ephemeral transfer service; restart invalidates every outstanding token.
    pub fn new(executor: BlockingExecutor, config: PublicConfig) -> Self {
        let capacity = 16.min(Semaphore::MAX_PERMITS);
        // SAFETY: capacity is bounded above by Tokio's maximum permit count.
        let uploads = Semaphore::new(capacity);
        Self {
            executor,
            config,
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            uploads: Arc::new(uploads),
        }
    }
    /// Authorize exact destinations, resolving the namespace to its stable UUID.
    pub async fn upload(&self, input: FsUploadInput) -> Result<TransferTicket, BlockingError> {
        let (namespace, paths) = self
            .executor
            .run(move |controller| controller.prepare_upload(input))
            .await?;
        self.issue(
            Grant::Upload { namespace, paths },
            Instant::now(),
            SystemTime::now(),
        )
        .map_err(BlockingError::Application)
    }
    /// Resolve literal/glob selectors before issuing a single-use tar URL.
    pub async fn download(&self, input: FsDownloadInput) -> Result<TransferTicket, BlockingError> {
        let selection = self
            .executor
            .run(move |controller| controller.prepare_download(input))
            .await?;
        self.issue(
            Grant::Download(selection),
            Instant::now(),
            SystemTime::now(),
        )
        .map_err(BlockingError::Application)
    }
    fn issue(
        &self,
        grant: Grant,
        now: Instant,
        wall: SystemTime,
    ) -> state_core::Result<TransferTicket> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| Error::new(ErrorCode::WorkerFailed, "transfer state unavailable"))?;
        pending.retain(|_, item| item.expires > now);
        if pending.len() >= MAX_TOKENS {
            return Err(Error::new(
                ErrorCode::Busy,
                "too many outstanding transfer tokens",
            ));
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let endpoint = match grant.direction() {
            Direction::Upload => "upload",
            Direction::Download => "download",
        };
        let url = format!(
            "{}{}/{endpoint}?token={token}",
            self.config.public_url.as_deref().unwrap_or(""),
            self.config.prefix
        );
        let files = grant.paths().to_vec();
        let expires = now
            .checked_add(TOKEN_LIFETIME)
            .ok_or_else(|| Error::new(ErrorCode::WorkerFailed, "transfer deadline overflow"))?;
        let expires_at = wall
            .checked_add(TOKEN_LIFETIME)
            .ok_or_else(|| Error::new(ErrorCode::WorkerFailed, "transfer clock overflow"))?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::new(ErrorCode::WorkerFailed, "clock precedes Unix epoch"))?
            .as_secs();
        pending.insert(token.clone(), Pending { expires, grant });
        Ok(TransferTicket {
            token,
            url,
            expires_at,
            files,
        })
    }
    fn claim(&self, token: &str, direction: Direction, now: Instant) -> Result<Grant, StatusCode> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        pending.retain(|_, item| item.expires > now);
        let item = pending.get(token).ok_or(StatusCode::UNAUTHORIZED)?;
        if item.grant.direction() != direction {
            return Err(StatusCode::UNAUTHORIZED);
        }
        pending
            .remove(token)
            .map(|item| item.grant)
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}
/// Reserved transfer routes. The outer server still enforces Host/Origin policy.
pub fn transfer_router(transfers: Transfers) -> Router {
    Router::new()
        .route("/upload", post(upload))
        .route("/download", get(download))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + MULTIPART_OVERHEAD))
        .with_state(transfers)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenQuery {
    token: String,
}

async fn upload(
    State(service): State<Transfers>,
    Query(query): Query<TokenQuery>,
    request: Request,
) -> Response {
    let _permit = match service.uploads.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let grant = match service.claim(&query.token, Direction::Upload, Instant::now()) {
        Ok(grant) => grant,
        Err(status) => return status.into_response(),
    };
    let Grant::Upload { namespace, paths } = grant else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut multipart = match Multipart::from_request(request, &()).await {
        Ok(multipart) => multipart,
        Err(error) => return error.into_response(),
    };
    let (directory_path, directory) = match tokio::task::spawn_blocking(|| {
        let directory = tempfile::tempdir()?;
        Ok::<_, io::Error>((
            directory.path().to_owned(),
            StagingDirectory(Some(directory)),
        ))
    })
    .await
    {
        Ok(Ok(directory)) => directory,
        _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut remaining: BTreeMap<_, _> = paths
        .paths()
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, path)| {
            (
                path.as_str().to_owned(),
                (path, directory_path.join(index.to_string())),
            )
        })
        .collect();
    let mut files = Vec::new();
    let mut total = 0usize;
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return error.into_response(),
        };
        let Some((path, temporary)) = field.name().and_then(|name| remaining.remove(name)) else {
            return (
                StatusCode::BAD_REQUEST,
                "unexpected or duplicate multipart field",
            )
                .into_response();
        };
        if field.file_name().is_none() {
            return (StatusCode::BAD_REQUEST, "each field must be a file upload").into_response();
        }
        let mut file = match tokio::fs::File::create(&temporary).await {
            Ok(file) => file,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        loop {
            let chunk = match field.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => return error.into_response(),
            };
            total = total.saturating_add(chunk.len());
            if total > MAX_UPLOAD_BYTES {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
            if file.write_all(&chunk).await.is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        if file.flush().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        drop(file);
        files.push((path, temporary));
    }
    if !remaining.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing multipart files").into_response();
    }
    match service
        .executor
        .run(move |controller| {
            // The directory owns staging files until the complete atomic import finishes.
            let _directory = directory;
            controller.import_files(
                &namespace,
                files.into_iter().map(|(path, temporary)| {
                    std::fs::read(temporary)
                        .map(|bytes| (path, bytes))
                        .map_err(|_| Error::new(ErrorCode::IoError, "cannot read staged upload"))
                }),
            )
        })
        .await
    {
        Ok(files) => (StatusCode::CREATED, Json(files)).into_response(),
        Err(error) => failure(error),
    }
}
// TempDir deletion is synchronous, including on early return or cancellation.
// This guard moves cleanup onto a blocking thread in every async drop path.
struct StagingDirectory(Option<tempfile::TempDir>);
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    runtime.spawn_blocking(move || drop(directory));
                }
                Err(_) => drop(directory),
            }
        }
    }
}

struct ArchiveWriter(mpsc::Sender<Result<Bytes, io::Error>>);
impl Write for ArchiveWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for chunk in bytes.chunks(64 * 1024) {
            self.0
                .blocking_send(Ok(Bytes::copy_from_slice(chunk)))
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "download client disconnected")
                })?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
async fn download(
    State(service): State<Transfers>,
    Query(query): Query<TokenQuery>,
    method: Method,
) -> Response {
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let selection = match service.claim(&query.token, Direction::Download, Instant::now()) {
        Ok(Grant::Download(selection)) => selection,
        Ok(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(status) => return status.into_response(),
    };
    // SAFETY: the bounded channel capacity is the nonzero literal 4.
    let (sender, mut receiver) = mpsc::channel(4);
    tokio::spawn(async move {
        let errors = sender.clone();
        let result = service
            .executor
            .run(move |controller| {
                let mut archive = tar::Builder::new(ArchiveWriter(sender));
                controller.export_files(&selection, |path, bytes| {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(bytes.len() as u64);
                    header.set_mode(0o644);
                    header.set_mtime(0);
                    header.set_cksum();
                    archive
                        .append_data(&mut header, path.archive_name(), bytes.as_slice())
                        .map_err(|_| Error::new(ErrorCode::IoError, "archive stream failed"))
                })?;
                archive
                    .finish()
                    .map_err(|_| Error::new(ErrorCode::IoError, "archive stream failed"))?;
                Ok(())
            })
            .await;
        if let Err(error) = result {
            tracing::debug!(%error, "download failed");
            let _ = errors.send(Err(io::Error::other("download failed"))).await;
        }
    });
    // Detect failures before the first archive bytes, without buffering the archive.
    let first = match receiver.recv().await {
        Some(Ok(bytes)) => bytes,
        _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let stream = futures_util::stream::once(async move { Ok::<_, io::Error>(first) })
        .chain(ReceiverStream::new(receiver));
    let mut response = Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/x-tar"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_static("attachment; filename=\"files.tar\""),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
fn failure(error: BlockingError) -> Response {
    match error {
        BlockingError::Application(error) => {
            let status = match error.code {
                ErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
                ErrorCode::LimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::NotFound => StatusCode::NOT_FOUND,
                ErrorCode::Busy | ErrorCode::Conflict => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, error.code.as_str()).into_response()
        }
        BlockingError::Task(error) => {
            tracing::error!(%error, "transfer task failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn service() -> Result<Transfers, Box<dyn std::error::Error>> {
        let store = state_store::Store::builder()
            .with_fs(state_fs::MemoryFs::new())
            .open("/state")?;
        let controller = state_core::Controller::new(
            state_domain::Store::from_storage(store)?,
            state_core::Runtime::embedded(),
        );
        Ok(Transfers::new(
            BlockingExecutor::new(controller),
            PublicConfig {
                public_url: Some("https://example.com".into()),
                prefix: "/prefix".into(),
            },
        ))
    }
    fn grant() -> Result<Grant, Box<dyn std::error::Error>> {
        Ok(Grant::Upload {
            namespace: "id".into(),
            paths: UploadPaths::parse(&["/folder/a".into()])?,
        })
    }
    #[test]
    fn tokens_expire_are_direction_bound_and_allow_one_claim()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = service()?;
        let now = Instant::now();
        let ticket = service.issue(grant()?, now, UNIX_EPOCH + Duration::from_secs(1000))?;
        assert_eq!(ticket.expires_at, 1600);
        assert!(
            ticket
                .url
                .starts_with("https://example.com/prefix/upload?token=")
        );
        assert!(matches!(
            service.claim(&ticket.token, Direction::Download, now),
            Err(StatusCode::UNAUTHORIZED)
        ));
        assert!(service.claim(&ticket.token, Direction::Upload, now).is_ok());
        assert!(matches!(
            service.claim(&ticket.token, Direction::Upload, now),
            Err(StatusCode::UNAUTHORIZED)
        ));
        let expired = service.issue(grant()?, now, UNIX_EPOCH)?;
        assert!(matches!(
            service.claim(&expired.token, Direction::Upload, now + TOKEN_LIFETIME),
            Err(StatusCode::UNAUTHORIZED)
        ));
        let path_only = Transfers::new(
            service.executor.clone(),
            PublicConfig {
                public_url: None,
                prefix: "/prefix".into(),
            },
        );
        assert!(
            path_only
                .issue(grant()?, now, UNIX_EPOCH)?
                .url
                .starts_with("/prefix/upload?token=")
        );
        Ok(())
    }
    #[test]
    fn concurrent_claims_have_one_winner() -> Result<(), Box<dyn std::error::Error>> {
        let service = service()?;
        let now = Instant::now();
        let ticket = service.issue(grant()?, now, UNIX_EPOCH)?;
        let outcomes = std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| service.claim(&ticket.token, Direction::Upload, now).is_ok())
                })
                .collect();
            tasks
                .into_iter()
                .map(|task| task.join())
                .collect::<Vec<_>>()
        });
        let winners: Result<Vec<_>, _> = outcomes.into_iter().collect();
        let winners = winners.map_err(|_| io::Error::other("claim thread failed"))?;
        assert_eq!(winners.into_iter().filter(|won| *won).count(), 1);
        Ok(())
    }
}
