//! Concurrent downloads with SHA-1 checks.
//!
//! Files are written to a sibling `*.part` path and renamed only after the
//! checksum matches. A file that is already present and matches is skipped.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::stream::{self, StreamExt, TryStreamExt};
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, IoResultExt, Result};
use crate::progress::{ProgressEvent, ProgressSink};

/// Largest single file Weathered will accept. Larger than any current client
/// jar or Java runtime file, and small enough to reject a runaway response.
pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Largest JSON document Weathered will buffer.
const MAX_JSON_BYTES: usize = 20 * 1024 * 1024;

/// One file to fetch.
#[derive(Debug, Clone)]
pub struct RemoteFile {
    /// Download URL. Empty when the file is expected to exist already.
    pub url: String,
    /// Final path.
    pub dest: PathBuf,
    /// Expected SHA-1, if the source published one.
    pub sha1: Option<String>,
    /// Expected SHA-512, when a loader profile published one.
    pub sha512: Option<String>,
    /// Expected size in bytes, if known.
    pub size: Option<u64>,
    /// Set the executable bit after a successful download (Unix runtimes).
    pub executable: bool,
}

/// How many files were fetched versus already valid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadStats {
    /// Files written by this call.
    pub downloaded: u64,
    /// Files left untouched because they already matched.
    pub skipped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Downloaded,
    Skipped,
}

/// HTTP client used for manifests, libraries, assets, and authentication.
pub fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("weathered-launcher/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .pool_max_idle_per_host(32)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|err| Error::message(format!("could not build HTTP client: {err}")))
}

/// SHA-1 of an in-memory buffer, lowercase hex.
pub fn sha1_hex(data: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data.as_ref());
    hex::encode(hasher.finalize())
}

/// Download `url` into memory. Intended for manifests and other JSON documents.
pub async fn get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    get_bytes_cancellable(client, url, &crate::cancel::CancelFlag::new()).await
}

/// [`get_bytes`] that returns as soon as `cancel` is raised.
pub async fn get_bytes_cancellable(
    client: &reqwest::Client,
    url: &str,
    cancel: &crate::cancel::CancelFlag,
) -> Result<Vec<u8>> {
    let mut last = None;
    for attempt in 1u32..=3 {
        cancel.check()?;
        let once = async {
            tokio::select! {
                biased;
                () = cancel.cancelled() => Err(Error::message("cancelled")),
                result = get_bytes_once(client, url) => result,
            }
        };
        match once.await {
            Ok(bytes) => return Ok(bytes),
            Err(err) if err.is_cancelled() => return Err(err),
            Err(err) if attempt < 3 && err.is_retryable() => {
                last = Some(err);
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(Error::message("cancelled")),
                    () = tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))) => {}
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| Error::message(format!("request to {url} failed"))))
}

async fn get_bytes_once(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client.get(url).send().await.map_err(|source| Error::Http {
        url: url.to_string(),
        source,
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::HttpStatus {
            url: url.to_string(),
            status: status.as_u16(),
            body: clip(&body, 300),
        });
    }
    let bytes = response.bytes().await.map_err(|source| Error::Http {
        url: url.to_string(),
        source,
    })?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(Error::message(format!(
            "response from {url} is larger than {MAX_JSON_BYTES} bytes"
        )));
    }
    Ok(bytes.to_vec())
}

/// Download every file, with at most `concurrency` requests in flight.
pub async fn download_all(
    client: &reqwest::Client,
    files: Vec<RemoteFile>,
    concurrency: usize,
    stage: &str,
    progress: &ProgressSink,
) -> Result<DownloadStats> {
    let total = files.len() as u64;
    if total == 0 {
        progress.emit(ProgressEvent {
            stage: stage.to_string(),
            completed: 0,
            total: 0,
            detail: "nothing to download".into(),
        });
        return Ok(DownloadStats::default());
    }

    let concurrency = concurrency.max(1);
    let completed = Arc::new(AtomicU64::new(0));
    let downloaded = Arc::new(AtomicU64::new(0));
    let skipped = Arc::new(AtomicU64::new(0));
    let stage_name = stage.to_string();
    let client = client.clone();
    let progress = progress.clone();

    stream::iter(files)
        .map(|file| {
            let client = client.clone();
            let progress = progress.clone();
            let completed = Arc::clone(&completed);
            let downloaded = Arc::clone(&downloaded);
            let skipped = Arc::clone(&skipped);
            let stage_name = stage_name.clone();
            async move {
                let outcome = download_with_retry(&client, &file).await?;
                match outcome {
                    Outcome::Downloaded => {
                        downloaded.fetch_add(1, Ordering::Relaxed);
                    }
                    Outcome::Skipped => {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                progress.emit(ProgressEvent {
                    stage: stage_name,
                    completed: done,
                    total,
                    detail: file.dest.display().to_string(),
                });
                Ok::<(), Error>(())
            }
        })
        .buffer_unordered(concurrency)
        .try_for_each(|()| async { Ok(()) })
        .await?;

    Ok(DownloadStats {
        downloaded: downloaded.load(Ordering::Relaxed),
        skipped: skipped.load(Ordering::Relaxed),
    })
}

async fn download_with_retry(client: &reqwest::Client, file: &RemoteFile) -> Result<Outcome> {
    if file_is_valid(file).await? {
        return Ok(Outcome::Skipped);
    }
    if file.url.is_empty() {
        return Err(Error::message(format!(
            "no download URL for {} and the file is not already present",
            file.dest.display()
        )));
    }
    let mut last = None;
    for attempt in 1u32..=3 {
        match download_once(client, file).await {
            Ok(()) => return Ok(Outcome::Downloaded),
            Err(err) if attempt < 3 && err.is_retryable() => {
                last = Some(err);
                tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| Error::message("download failed")))
}

async fn file_is_valid(file: &RemoteFile) -> Result<bool> {
    let dest = file.dest.clone();
    let expected = file.sha1.clone();
    let size = file.size;
    tokio::task::spawn_blocking(move || -> Result<bool> {
        let meta = match std::fs::metadata(&dest) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(Error::io(&dest, err)),
        };
        if !meta.is_file() {
            return Ok(false);
        }
        if let Some(size) = size
            && meta.len() != size
        {
            return Ok(false);
        }
        if let Some(expected) = expected {
            let actual = hash_file(&dest)?;
            return Ok(actual.eq_ignore_ascii_case(&expected));
        }
        Ok(meta.len() > 0)
    })
    .await
    .map_err(|err| Error::message(format!("background task failed: {err}")))?
}

async fn download_once(client: &reqwest::Client, file: &RemoteFile) -> Result<()> {
    let response = client
        .get(&file.url)
        .send()
        .await
        .map_err(|source| Error::Http {
            url: file.url.clone(),
            source,
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::HttpStatus {
            url: file.url.clone(),
            status: status.as_u16(),
            body: clip(&body, 300),
        });
    }

    let tmp = part_path(&file.dest);
    if let Some(parent) = tmp.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await.io_at(parent)?;
    }
    let mut output = tokio::fs::File::create(&tmp).await.io_at(&tmp)?;
    let mut hasher = Sha1::new();
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| Error::Http {
            url: file.url.clone(),
            source,
        })?;
        downloaded += chunk.len() as u64;
        if downloaded > MAX_FILE_BYTES {
            drop(output);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(Error::message(format!(
                "download of {} exceeded {MAX_FILE_BYTES} bytes",
                file.url
            )));
        }
        hasher.update(&chunk);
        output.write_all(&chunk).await.io_at(&tmp)?;
    }
    output.flush().await.io_at(&tmp)?;
    drop(output);

    let actual = hex::encode(hasher.finalize());
    if let Some(expected) = &file.sha1 {
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(Error::Checksum {
                path: file.dest.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    } else if let Some(size) = file.size
        && downloaded != size
    {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(Error::message(format!(
            "size mismatch for {}: expected {size} bytes, got {downloaded}",
            file.dest.display()
        )));
    }

    if file.executable {
        set_executable(&tmp).io_at(&tmp)?;
    }
    replace_file(&tmp, &file.dest).await
}

/// Write `bytes` to `path` via a temporary file.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await.io_at(parent)?;
    }
    let tmp = part_path(path);
    tokio::fs::write(&tmp, bytes).await.io_at(&tmp)?;
    replace_file(&tmp, path).await
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).io_at(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).io_at(path)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

async fn replace_file(tmp: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await.io_at(parent)?;
    }
    #[cfg(windows)]
    if tokio::fs::try_exists(dest).await.unwrap_or(false) {
        tokio::fs::remove_file(dest).await.io_at(dest)?;
    }
    tokio::fs::rename(tmp, dest).await.io_at(dest)
}

fn set_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        std::fs::set_permissions(path, permissions)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let clipped: String = flat.chars().take(max).collect();
    format!("{clipped}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_of_abc_matches_the_known_vector() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }
}
