//! Content-addressed store.
//!
//! Every downloaded file is stored once at `store/objects/<sha512[0..2]>/<sha512>`.
//! An index records `sha1 -> sha512` and `url -> sha512` so a later download can
//! see that the bytes are already present before opening a connection.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha512};

use futures::stream::{StreamExt, TryStreamExt};

use crate::cancel::CancelFlag;
use crate::download::MAX_FILE_BYTES;
use crate::error::{Error, IoResultExt, Result};
use crate::progress::{ProgressEvent, ProgressSink};

/// One object written into the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    /// Lowercase SHA-512 of the bytes.
    pub sha512: String,
    /// Lowercase SHA-1 of the bytes.
    pub sha1: String,
    /// Absolute path of the object file.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// The object file was already present.
    pub cached: bool,
}

/// A file the store should fetch, skipping the network when an index hits.
#[derive(Debug, Clone)]
pub struct BlobRequest {
    /// Download URL. Empty when the caller only wants an index lookup.
    pub url: String,
    /// Expected SHA-1, when the source published one.
    pub sha1: Option<String>,
    /// Expected SHA-512, when the source published one.
    pub sha512: Option<String>,
    /// Expected size, when known. Used for the overall progress total.
    pub size: Option<u64>,
}

/// How many blobs were already in the store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FetchStats {
    /// Objects created by this call.
    pub downloaded: u64,
    /// Objects reused from the store.
    pub cached: u64,
}

/// URL index record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UrlRecord {
    /// Lowercase SHA-512.
    pub sha512: String,
    /// Lowercase SHA-1.
    pub sha1: String,
    /// URL the bytes were fetched from.
    pub url: String,
}

/// Root of `store/`.
#[derive(Debug, Clone)]
pub struct Store {
    /// `store/` directory.
    pub root: PathBuf,
}

impl Store {
    /// Open or create a store at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("objects")).io_at(root.join("objects"))?;
        std::fs::create_dir_all(root.join("index").join("sha1")).io_at(root.join("index"))?;
        std::fs::create_dir_all(root.join("tmp")).io_at(root.join("tmp"))?;
        Ok(Self { root })
    }

    /// `objects/<sha512[0..2]>/<sha512>`.
    pub fn object_path(&self, sha512: &str) -> Result<PathBuf> {
        let sha512 = normalize_hex(sha512, 128, "sha512")?;
        Ok(self.root.join("objects").join(&sha512[..2]).join(&sha512))
    }

    /// SHA-512 recorded for `sha1`, when the index and the object both exist.
    pub fn lookup_sha1(&self, sha1: &str) -> Result<Option<String>> {
        let sha1 = match normalize_hex(sha1, 40, "sha1") {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let path = self.sha1_index(&sha1);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).io_at(&path)?;
        let sha512 = text.trim();
        if sha512.len() != 128 || !self.object_path(sha512)?.is_file() {
            return Ok(None);
        }
        Ok(Some(sha512.to_string()))
    }

    /// Record for `url`, when the index and the object both exist.
    pub fn lookup_url(&self, url: &str) -> Result<Option<UrlRecord>> {
        let path = self.url_index(url);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).io_at(&path)?;
        let record: UrlRecord = serde_json::from_slice(&bytes)
            .map_err(|err| Error::json(format!("url index {}", path.display()), err))?;
        if !self.object_path(&record.sha512)?.is_file() {
            return Ok(None);
        }
        Ok(Some(record))
    }

    /// Store `bytes`, verifying `sha1` when it is provided.
    ///
    /// A second call with the same bytes does not write another object file.
    pub fn ingest(
        &self,
        bytes: &[u8],
        expected_sha1: Option<&str>,
        expected_sha512: Option<&str>,
        url: Option<&str>,
    ) -> Result<StoredObject> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(Error::message(format!(
                "object is larger than {MAX_FILE_BYTES} bytes"
            )));
        }
        let sha1 = hex::encode(Sha1::digest(bytes));
        let sha512 = hex::encode(Sha512::digest(bytes));
        if let Some(expected) = expected_sha1 {
            let expected = normalize_hex(expected, 40, "sha1")?;
            if !sha1.eq_ignore_ascii_case(&expected) {
                return Err(Error::Checksum {
                    path: self
                        .object_path(&sha512)
                        .unwrap_or_else(|_| PathBuf::from("store")),
                    expected,
                    actual: sha1,
                });
            }
        }
        if let Some(expected) = expected_sha512 {
            let expected = normalize_hex(expected, 128, "sha512")?;
            if sha512 != expected {
                return Err(Error::Checksum {
                    path: self
                        .object_path(&sha512)
                        .unwrap_or_else(|_| PathBuf::from("store")),
                    expected,
                    actual: sha512,
                });
            }
        }
        self.commit_object(&sha512, &sha1, bytes, url)
    }

    /// Copy an object to `dest`. The store file is left in place.
    pub fn copy_to(&self, sha512: &str, dest: &Path) -> Result<()> {
        let src = self.object_path(sha512)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        std::fs::copy(&src, dest).io_at(dest)?;
        Ok(())
    }

    /// Hard-link `sha512` to `dest`, or copy when the link cannot be created.
    pub fn link_to(&self, sha512: &str, dest: &Path) -> Result<()> {
        if dest.is_file() {
            return Ok(());
        }
        let src = self.object_path(sha512)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        if std::fs::hard_link(&src, dest).is_ok() {
            return Ok(());
        }
        std::fs::copy(&src, dest).io_at(dest)?;
        Ok(())
    }

    /// Number of object files currently stored.
    pub fn object_count(&self) -> usize {
        let root = self.root.join("objects");
        let Ok(prefixes) = std::fs::read_dir(&root) else {
            return 0;
        };
        let mut count = 0;
        for prefix in prefixes.flatten() {
            if !prefix.path().is_dir() {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(prefix.path()) {
                count += files.filter_map(|entry| entry.ok()).count();
            }
        }
        count
    }

    /// Download any blobs that are not already indexed, with one byte total.
    pub async fn fetch_all(
        &self,
        client: &reqwest::Client,
        requests: Vec<BlobRequest>,
        concurrency: usize,
        progress: &ProgressSink,
        cancel: &CancelFlag,
    ) -> Result<FetchStats> {
        let mut pending = Vec::new();
        let mut cached = 0u64;
        let mut total_bytes = 0u64;
        let mut done_bytes = 0u64;
        for request in requests {
            cancel.check()?;
            if request.url.is_empty() && request.sha1.is_none() && request.sha512.is_none() {
                continue;
            }
            if let Some(object) = self.cached_object(&request)? {
                cached += 1;
                let size = request.size.unwrap_or(object.size);
                total_bytes += size;
                done_bytes += size;
                continue;
            }
            total_bytes += request.size.unwrap_or(0);
            pending.push(request);
        }
        let (shown_done, shown_total, detail) = opening_progress(done_bytes, total_bytes, &pending);
        emit_overall(progress, shown_done, shown_total, cached, &detail);
        if pending.is_empty() {
            return Ok(FetchStats {
                downloaded: 0,
                cached,
            });
        }

        let concurrency = concurrency.max(1);
        let downloaded = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(shown_done));
        let total = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(shown_total));
        let client = client.clone();
        let store = self.clone();
        let progress = progress.clone();
        let cancel = cancel.clone();

        futures::stream::iter(pending)
            .map(|request| {
                let client = client.clone();
                let store = store.clone();
                let progress = progress.clone();
                let downloaded = std::sync::Arc::clone(&downloaded);
                let done = std::sync::Arc::clone(&done);
                let total = std::sync::Arc::clone(&total);
                let cancel = cancel.clone();
                async move {
                    cancel.check()?;
                    let known = request.size.unwrap_or(0);
                    let object = tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Err(Error::message("cancelled")),
                        result = store.fetch_one(&client, &request) => result?,
                    };
                    if !object.cached {
                        downloaded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let size = object.size;
                    if known == 0 {
                        total.fetch_add(
                            size.saturating_sub(1),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    let added = if known == 0 { size.max(1) } else { known };
                    let completed =
                        done.fetch_add(added, std::sync::atomic::Ordering::Relaxed) + added;
                    emit_overall(
                        &progress,
                        completed,
                        total.load(std::sync::atomic::Ordering::Relaxed),
                        0,
                        &request.url,
                    );
                    Ok::<(), Error>(())
                }
            })
            .buffer_unordered(concurrency)
            .try_for_each(|()| async { Ok(()) })
            .await?;

        Ok(FetchStats {
            downloaded: downloaded.load(std::sync::atomic::Ordering::Relaxed),
            cached,
        })
    }

    fn cached_object(&self, request: &BlobRequest) -> Result<Option<StoredObject>> {
        if let Some(sha512) = request.sha512.as_deref() {
            if let Ok(path) = self.object_path(sha512) {
                if path.is_file() {
                    let size = std::fs::metadata(&path).io_at(&path)?.len();
                    let sha1 = request
                        .sha1
                        .clone()
                        .unwrap_or_else(|| lookup_sha1_of_sha512(self, sha512).unwrap_or_default());
                    if !request.url.is_empty() && !sha1.is_empty() {
                        self.write_url_index(&request.url, sha512, &sha1)?;
                    }
                    return Ok(Some(StoredObject {
                        sha512: normalize_hex(sha512, 128, "sha512")?,
                        sha1,
                        path,
                        size,
                        cached: true,
                    }));
                }
            }
        }
        if let Some(sha1) = request.sha1.as_deref() {
            if let Some(sha512) = self.lookup_sha1(sha1)? {
                let path = self.object_path(&sha512)?;
                let size = std::fs::metadata(&path).io_at(&path)?.len();
                if !request.url.is_empty() {
                    self.write_url_index(&request.url, &sha512, sha1)?;
                }
                return Ok(Some(StoredObject {
                    sha512,
                    sha1: normalize_hex(sha1, 40, "sha1")?,
                    path,
                    size,
                    cached: true,
                }));
            }
        }
        if !request.url.is_empty() {
            if let Some(record) = self.lookup_url(&request.url)? {
                let path = self.object_path(&record.sha512)?;
                let size = std::fs::metadata(&path).io_at(&path)?.len();
                return Ok(Some(StoredObject {
                    sha512: record.sha512,
                    sha1: record.sha1,
                    path,
                    size,
                    cached: true,
                }));
            }
        }
        Ok(None)
    }

    async fn fetch_one(
        &self,
        client: &reqwest::Client,
        request: &BlobRequest,
    ) -> Result<StoredObject> {
        if let Some(object) = self.cached_object(request)? {
            return Ok(object);
        }
        if request.url.is_empty() {
            return Err(Error::message(
                "file is not in the store and no download URL was provided",
            ));
        }
        let response = client
            .get(&request.url)
            .send()
            .await
            .map_err(|source| Error::Http {
                url: request.url.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::HttpStatus {
                url: request.url.clone(),
                status: status.as_u16(),
                body: body.chars().take(300).collect(),
            });
        }
        let mut sha1 = Sha1::new();
        let mut sha512 = Sha512::new();
        let mut downloaded = 0u64;
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| Error::Http {
                url: request.url.clone(),
                source,
            })?;
            downloaded += chunk.len() as u64;
            if downloaded > MAX_FILE_BYTES {
                return Err(Error::message(format!(
                    "download of {} exceeded {MAX_FILE_BYTES} bytes",
                    request.url
                )));
            }
            sha1.update(&chunk);
            sha512.update(&chunk);
            bytes.extend_from_slice(&chunk);
        }
        let actual_sha1 = hex::encode(sha1.finalize());
        let actual_sha512 = hex::encode(sha512.finalize());
        if let Some(expected) = &request.sha1 {
            let expected = normalize_hex(expected, 40, "sha1")?;
            if actual_sha1 != expected {
                return Err(Error::Checksum {
                    path: PathBuf::from(&request.url),
                    expected,
                    actual: actual_sha1,
                });
            }
        }
        if let Some(expected) = &request.sha512 {
            let expected = normalize_hex(expected, 128, "sha512")?;
            if actual_sha512 != expected {
                return Err(Error::Checksum {
                    path: PathBuf::from(&request.url),
                    expected,
                    actual: actual_sha512,
                });
            }
        }
        let url = request.url.clone();
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.commit_object(&actual_sha512, &actual_sha1, &bytes, Some(url.as_str()))
        })
        .await
        .map_err(|err| Error::message(format!("background task failed: {err}")))?
    }

    fn commit_object(
        &self,
        sha512: &str,
        sha1: &str,
        bytes: &[u8],
        url: Option<&str>,
    ) -> Result<StoredObject> {
        let path = self.object_path(sha512)?;
        let cached = path.is_file();
        if !cached {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).io_at(parent)?;
            }
            let tmp = self.root.join("tmp").join(format!("{sha512}.part"));
            {
                let mut file = std::fs::File::create(&tmp).io_at(&tmp)?;
                file.write_all(bytes).io_at(&tmp)?;
                file.flush().io_at(&tmp)?;
            }
            if path.is_file() {
                let _ = std::fs::remove_file(&tmp);
            } else {
                std::fs::rename(&tmp, &path).io_at(&path)?;
            }
        }
        self.write_sha1_index(sha1, sha512)?;
        if let Some(url) = url.filter(|url| !url.is_empty()) {
            self.write_url_index(url, sha512, sha1)?;
        }
        Ok(StoredObject {
            sha512: sha512.to_string(),
            sha1: sha1.to_string(),
            path,
            size: bytes.len() as u64,
            cached,
        })
    }

    fn write_sha1_index(&self, sha1: &str, sha512: &str) -> Result<()> {
        let sha1 = normalize_hex(sha1, 40, "sha1")?;
        let path = self.sha1_index(&sha1);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        std::fs::write(&path, format!("{sha512}\n")).io_at(&path)
    }

    fn write_url_index(&self, url: &str, sha512: &str, sha1: &str) -> Result<()> {
        let path = self.url_index(url);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        let record = UrlRecord {
            sha512: sha512.to_string(),
            sha1: sha1.to_string(),
            url: url.to_string(),
        };
        let bytes = serde_json::to_vec(&record).map_err(|err| Error::json("url index", err))?;
        std::fs::write(&path, bytes).io_at(&path)
    }

    fn sha1_index(&self, sha1: &str) -> PathBuf {
        self.root.join("index").join("sha1").join(sha1)
    }

    fn url_index(&self, url: &str) -> PathBuf {
        let key = hex::encode(Sha256::digest(url.as_bytes()));
        self.root
            .join("index")
            .join("url")
            .join(&key[..2])
            .join(key)
    }
}

fn lookup_sha1_of_sha512(store: &Store, sha512: &str) -> Option<String> {
    let dir = store.root.join("index").join("sha1");
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if text.trim() == sha512 {
            return entry.file_name().into_string().ok();
        }
    }
    None
}

/// Progress before any pending transfer starts.
///
/// Cached files count as finished. Pending files with no size still leave the
/// total ahead of the completed count, so the bar does not read 100% while
/// work remains.
fn opening_progress(done: u64, known_total: u64, pending: &[BlobRequest]) -> (u64, u64, String) {
    if pending.is_empty() {
        return (done, known_total, "already stored".into());
    }
    let unknown = pending
        .iter()
        .filter(|item| item.size.unwrap_or(0) == 0)
        .count() as u64;
    let mut total = known_total.saturating_add(unknown);
    if total <= done {
        total = done.saturating_add(unknown.max(1));
    }
    let detail = pending
        .first()
        .map(|item| {
            let name = item.url.rsplit(['/', '?']).next().unwrap_or("");
            if name.is_empty() {
                "downloading".into()
            } else {
                name.to_string()
            }
        })
        .unwrap_or_else(|| "downloading".into());
    (done, total, detail)
}

fn emit_overall(progress: &ProgressSink, completed: u64, total: u64, _cached: u64, detail: &str) {
    let detail = detail
        .rsplit(['/', '?'])
        .next()
        .unwrap_or(detail)
        .to_string();
    progress.emit(ProgressEvent {
        stage: "download".into(),
        completed,
        total,
        detail,
    });
}

fn normalize_hex(value: &str, len: usize, name: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != len || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(Error::message(format!("invalid {name} '{value}'")));
    }
    Ok(value)
}

/// SHA-512 of an in-memory buffer, lowercase hex.
pub fn sha512_hex(data: impl AsRef<[u8]>) -> String {
    hex::encode(Sha512::digest(data.as_ref()))
}

/// Read a file while hashing it. Used by tests and local imports.
pub fn hash_file_sha1(path: &Path) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (PathBuf, Store) {
        let root = std::env::temp_dir().join(format!(
            "weathered-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::open(&root).unwrap();
        (root, store)
    }

    #[test]
    fn object_path_uses_the_sha512_prefix() {
        let (root, store) = temp_store();
        let sha = sha512_hex(b"abc");
        assert_eq!(
            sha,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        let path = store.object_path(&sha).unwrap();
        assert_eq!(path, root.join("objects").join("dd").join(&sha));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn identical_bytes_are_stored_once_and_indexed_by_sha1_and_url() {
        let (root, store) = temp_store();
        let url = "https://example.test/client.jar";
        let first = store
            .ingest(
                b"abc",
                Some("a9993e364706816aba3e25717850c26c9cd0d89d"),
                None,
                Some(url),
            )
            .unwrap();
        assert!(!first.cached);
        assert_eq!(store.object_count(), 1);
        let second = store
            .ingest(
                b"abc",
                Some("a9993e364706816aba3e25717850c26c9cd0d89d"),
                None,
                Some(url),
            )
            .unwrap();
        assert!(second.cached);
        assert_eq!(store.object_count(), 1);
        assert_eq!(first.sha512, second.sha512);
        assert_eq!(
            store
                .lookup_sha1("A9993E364706816ABA3E25717850C26C9CD0D89D")
                .unwrap()
                .as_deref(),
            Some(first.sha512.as_str())
        );
        let record = store.lookup_url(url).unwrap().unwrap();
        assert_eq!(record.sha512, first.sha512);
        assert_eq!(record.url, url);
        let cached = store
            .cached_object(&BlobRequest {
                url: url.into(),
                sha1: Some(first.sha1.clone()),
                sha512: None,
                size: Some(3),
            })
            .unwrap()
            .unwrap();
        assert!(cached.cached);
        assert!(store
            .ingest(
                b"nope",
                Some("a9993e364706816aba3e25717850c26c9cd0d89d"),
                None,
                None
            )
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn unsized_pending_work_does_not_look_finished() {
        let (root, store) = temp_store();
        let stored = store
            .ingest(
                b"abc",
                Some("a9993e364706816aba3e25717850c26c9cd0d89d"),
                None,
                Some("https://example.test/cached"),
            )
            .unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&events);
        let sink = ProgressSink::new(move |event| {
            recorded.lock().unwrap().push(event);
        });
        let client = crate::download::build_http_client().unwrap();
        let pending = store
            .fetch_all(
                &client,
                vec![
                    BlobRequest {
                        url: "https://example.test/cached".into(),
                        sha1: Some(stored.sha1),
                        sha512: None,
                        size: Some(3),
                    },
                    BlobRequest {
                        url: String::new(),
                        sha1: None,
                        sha512: Some("ab".repeat(64)),
                        size: None,
                    },
                ],
                1,
                &sink,
                &crate::cancel::CancelFlag::new(),
            )
            .await;
        assert!(pending.is_err());
        let opening = events.lock().unwrap().first().cloned().unwrap();
        assert_ne!(opening.detail, "already stored");
        assert!(opening.completed < opening.total);

        events.lock().unwrap().clear();
        store
            .fetch_all(
                &client,
                vec![BlobRequest {
                    url: "https://example.test/cached".into(),
                    sha1: Some("a9993e364706816aba3e25717850c26c9cd0d89d".into()),
                    sha512: None,
                    size: Some(3),
                }],
                1,
                &sink,
                &crate::cancel::CancelFlag::new(),
            )
            .await
            .unwrap();
        let finished = events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(finished.detail, "already stored");
        assert_eq!(finished.completed, finished.total);
        let _ = std::fs::remove_dir_all(root);
    }
}
