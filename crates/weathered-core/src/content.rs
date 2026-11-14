//! Download vanilla versions and loader profiles into the shared store.
//!
//! Nothing is written into an instance here. Instance creation copies what it
//! needs out of the store afterwards.

use serde_json::Value;

use crate::assets::{self, parse_asset_index};
use crate::catalog::{self, CatalogItem};
use crate::download::{self, get_bytes};
use crate::error::{Error, Result};
use crate::java::{self, JavaMode};
use crate::library::plan_libraries;
use crate::manifest;
use crate::paths::DataLayout;
use crate::platform::OsInfo;
use crate::progress::ProgressSink;
use crate::rules::FeatureSet;
use crate::store::{BlobRequest, FetchStats, Store};
use crate::version::{self, VersionInfo};

const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
const QUILT_META: &str = "https://meta.quiltmc.org/v3/versions/loader";
const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

/// Which profile to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    /// Minecraft itself.
    Vanilla,
    /// Fabric loader, plus its Minecraft version.
    Fabric,
    /// Quilt loader, plus its Minecraft version.
    Quilt,
    /// Forge installer and its libraries. Launching is not supported yet.
    Forge,
    /// NeoForge installer and its libraries. Launching is not supported yet.
    NeoForge,
}

impl Loader {
    /// Parse `vanilla`, `fabric`, `quilt`, `forge`, or `neoforge`.
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "vanilla" | "none" => Ok(Self::Vanilla),
            "fabric" => Ok(Self::Fabric),
            "quilt" => Ok(Self::Quilt),
            "forge" => Ok(Self::Forge),
            "neoforge" => Ok(Self::NeoForge),
            other => Err(Error::message(format!(
                "unknown loader '{other}'. Use vanilla, fabric, quilt, forge, or neoforge"
            ))),
        }
    }

    /// Stable name stored in the catalog.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }
}

/// What [`download`] should fetch.
#[derive(Debug, Clone, Copy)]
pub struct DownloadRequest<'a> {
    /// Version id, or a Forge/NeoForge coordinate.
    pub version: &'a str,
    /// Which loader to attach. Vanilla downloads only Minecraft.
    pub loader: Loader,
    /// Explicit loader build. Empty selects the newest stable Fabric or Quilt loader.
    pub loader_version: Option<&'a str>,
    /// OS used to choose libraries.
    pub os: &'a OsInfo,
    /// Maximum concurrent downloads.
    pub concurrency: usize,
    /// Whether to download a Mojang Java runtime.
    pub java_mode: JavaMode,
}

/// Download `version` and, when requested, a loader profile.
///
/// `loader_version` selects a Fabric, Quilt, Forge, or NeoForge build. When it
/// is empty, Fabric and Quilt use the newest stable loader for `version`.
pub async fn download(
    client: &reqwest::Client,
    layout: &DataLayout,
    request: DownloadRequest<'_>,
    progress: &ProgressSink,
) -> Result<CatalogItem> {
    match request.loader {
        Loader::Vanilla => download_vanilla(client, layout, &request, progress).await,
        Loader::Fabric => {
            download_meta_loader(client, layout, &request, FABRIC_META, "fabric", progress).await
        }
        Loader::Quilt => {
            download_meta_loader(client, layout, &request, QUILT_META, "quilt", progress).await
        }
        Loader::Forge => {
            download_installer(
                client,
                layout,
                &request,
                FORGE_MAVEN,
                "forge",
                "forge",
                progress,
            )
            .await
        }
        Loader::NeoForge => {
            download_installer(
                client,
                layout,
                &request,
                NEOFORGE_MAVEN,
                "neoforge",
                "neoforge",
                progress,
            )
            .await
        }
    }
}

async fn download_vanilla(
    client: &reqwest::Client,
    layout: &DataLayout,
    request: &DownloadRequest<'_>,
    progress: &ProgressSink,
) -> Result<CatalogItem> {
    let store = Store::open(layout.store_root())?;
    let meta = layout.meta();
    let fetched = manifest::fetch_manifest(client, &meta.manifest_cache()).await?;
    let version_id = fetched.manifest.resolve_id(request.version).to_string();
    let resolved =
        version::load_resolved(client, &meta, &fetched.manifest, &version_id, true).await?;
    let json_path = meta.version_json(&resolved.info.id);
    let json_bytes = tokio::fs::read(&json_path)
        .await
        .map_err(|err| Error::io(&json_path, err))?;
    let stored_json = store.ingest(&json_bytes, None, None, None)?;
    let client_sha1 = client_sha1(&resolved.info)?;
    let mut requests = version_blobs(&resolved.info, request.os)?;
    let asset_count = queue_assets(client, layout, &store, &resolved.info, &mut requests).await?;
    let _stats = store
        .fetch_all(client, requests, request.concurrency, progress)
        .await?;
    if let Some(index_ref) = resolved.info.asset_index.clone() {
        if let Some(sha512) = store.lookup_sha1(&index_ref.sha1)? {
            let index_path = store.object_path(&sha512)?;
            let index_bytes =
                std::fs::read(&index_path).map_err(|err| Error::io(&index_path, err))?;
            publish_assets(&store, layout, &index_ref.id, &index_bytes)?;
        }
    }
    if request.java_mode != JavaMode::Skip {
        let _ = java::resolve_java(
            client,
            &layout.shared_assets(),
            java::JavaRequest {
                spec: resolved.info.java_version.as_ref(),
                explicit: None,
                mode: request.java_mode,
                os: request.os,
                concurrency: request.concurrency,
            },
            progress,
        )
        .await?;
    }
    let item = CatalogItem {
        id: format!("vanilla-{}", resolved.info.id),
        kind: "vanilla".into(),
        name: resolved.info.id.clone(),
        game_version: resolved.info.id.clone(),
        loader_version: String::new(),
        version_id: resolved.info.id,
        version_json_sha512: stored_json.sha512,
        parent_json_sha512: String::new(),
        parent_id: String::new(),
        client_sha1,
        launchable: true,
        note: format!("{asset_count} asset objects indexed"),
    };
    item.save(layout)?;
    Ok(item)
}

async fn download_meta_loader(
    client: &reqwest::Client,
    layout: &DataLayout,
    request: &DownloadRequest<'_>,
    meta_root: &str,
    kind: &str,
    progress: &ProgressSink,
) -> Result<CatalogItem> {
    let vanilla_request = DownloadRequest {
        loader: Loader::Vanilla,
        ..*request
    };
    let vanilla = download_vanilla(client, layout, &vanilla_request, progress).await?;
    let list_url = format!("{meta_root}/{}", vanilla.game_version);
    let list_bytes = get_bytes(client, &list_url).await?;
    let list: Value =
        serde_json::from_slice(&list_bytes).map_err(|err| Error::json("loader list", err))?;
    let selected = catalog::select_loader_version(&list, request.loader_version)?;
    let profile_url = format!(
        "{meta_root}/{}/{selected}/profile/json",
        vanilla.game_version
    );
    let profile_bytes = get_bytes(client, &profile_url).await?;
    let profile = version::parse_version(&profile_bytes)?;
    let store = Store::open(layout.store_root())?;
    let stored_json = store.ingest(&profile_bytes, None, None, Some(&profile_url))?;
    let requests = version_blobs(&profile, request.os)?;
    store
        .fetch_all(client, requests, request.concurrency, progress)
        .await?;
    let item = CatalogItem {
        id: format!("{kind}-{}-{selected}", vanilla.game_version),
        kind: kind.into(),
        name: format!("{kind} {} {selected}", vanilla.game_version),
        game_version: vanilla.game_version,
        loader_version: selected,
        version_id: profile.id,
        version_json_sha512: stored_json.sha512,
        parent_json_sha512: vanilla.version_json_sha512,
        parent_id: vanilla.version_id,
        client_sha1: vanilla.client_sha1,
        launchable: true,
        note: String::new(),
    };
    item.save(layout)?;
    Ok(item)
}

async fn download_installer(
    client: &reqwest::Client,
    layout: &DataLayout,
    request: &DownloadRequest<'_>,
    maven: &str,
    artifact: &str,
    kind: &str,
    progress: &ProgressSink,
) -> Result<CatalogItem> {
    let version = request.version.trim();
    if version.is_empty() {
        return Err(Error::message(format!(
            "pass the full {kind} version, such as the one published on its Maven feed"
        )));
    }
    let url = format!("{maven}/{version}/{artifact}-{version}-installer.jar");
    let store = Store::open(layout.store_root())?;
    let stats: FetchStats = store
        .fetch_all(
            client,
            vec![BlobRequest {
                url: url.clone(),
                sha1: None,
                sha512: None,
                size: None,
            }],
            4,
            progress,
        )
        .await?;
    let _ = stats;
    let record = store
        .lookup_url(&url)?
        .ok_or_else(|| Error::message(format!("{kind} installer was not stored")))?;
    let installer_path = store.object_path(&record.sha512)?;
    let version_json = read_zip_file(&installer_path, "version.json")?;
    let profile = version::parse_version(&version_json)?;
    let stored_json = store.ingest(&version_json, None, None, None)?;
    let game_version = version
        .split_once('-')
        .map(|(game, _)| game)
        .unwrap_or(version)
        .to_string();
    let requests = version_blobs(&profile, &OsInfo::current())?;
    store.fetch_all(client, requests, 16, progress).await?;
    let client_sha1 = profile
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.client.as_ref())
        .and_then(|artifact| artifact.sha1.clone())
        .unwrap_or_default();
    let item = CatalogItem {
        id: format!("{kind}-{version}"),
        kind: kind.into(),
        name: format!("{kind} {version}"),
        game_version,
        loader_version: version.to_string(),
        version_id: profile.id,
        version_json_sha512: stored_json.sha512,
        parent_json_sha512: String::new(),
        parent_id: String::new(),
        client_sha1,
        launchable: false,
        note: format!(
            "{kind} installer and libraries are in the store. The processor step that patches the client is not run, so this profile cannot be launched yet."
        ),
    };
    item.save(layout)?;
    Ok(item)
}

fn version_blobs(version: &VersionInfo, os: &OsInfo) -> Result<Vec<BlobRequest>> {
    let mut requests = Vec::new();
    if let Some(artifact) = version
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.client.as_ref())
    {
        if let Some(url) = artifact.url() {
            requests.push(BlobRequest {
                url: url.to_string(),
                sha1: artifact.sha1.clone(),
                sha512: None,
                size: artifact.size,
            });
        }
    }
    let libraries = version.libraries.clone().unwrap_or_default();
    let plan = plan_libraries(
        &libraries,
        std::path::Path::new("libraries"),
        os,
        &FeatureSet::default(),
    )?;
    for file in plan.downloads {
        requests.push(BlobRequest {
            url: file.url,
            sha1: file.sha1,
            sha512: None,
            size: file.size,
        });
    }
    if let Some(logging) = version
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
    {
        requests.push(BlobRequest {
            url: logging.file.url.clone(),
            sha1: Some(logging.file.sha1.clone()),
            sha512: None,
            size: Some(logging.file.size),
        });
    }
    Ok(dedup(requests))
}

async fn queue_assets(
    client: &reqwest::Client,
    layout: &DataLayout,
    store: &Store,
    version: &VersionInfo,
    requests: &mut Vec<BlobRequest>,
) -> Result<usize> {
    let Some(index_ref) = version.asset_index.as_ref() else {
        return Ok(0);
    };
    let bytes = get_bytes(client, &index_ref.url).await?;
    let actual = download::sha1_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&index_ref.sha1) {
        return Err(Error::Checksum {
            path: layout.shared_assets().asset_index(&index_ref.id),
            expected: index_ref.sha1.clone(),
            actual,
        });
    }
    store.ingest(&bytes, Some(&index_ref.sha1), None, Some(&index_ref.url))?;
    let index = parse_asset_index(&bytes)?;
    let count = index.objects.len();
    let planned = assets::plan_asset_downloads(&index, std::path::Path::new("objects"))?;
    for file in planned {
        requests.push(BlobRequest {
            url: file.url,
            sha1: file.sha1,
            sha512: None,
            size: file.size,
        });
    }
    Ok(count)
}

/// Link asset objects from the store into the shared SHA-1 asset tree.
pub fn publish_assets(
    store: &Store,
    layout: &DataLayout,
    index_id: &str,
    index_bytes: &[u8],
) -> Result<()> {
    let paths = layout.shared_assets();
    let index_path = paths.asset_index(index_id);
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| Error::io(parent, err))?;
    }
    std::fs::write(&index_path, index_bytes).map_err(|err| Error::io(&index_path, err))?;
    let index = parse_asset_index(index_bytes)?;
    for object in index.objects.values() {
        let Some(sha512) = store.lookup_sha1(&object.hash)? else {
            continue;
        };
        let dest = paths.asset_object(&object.hash);
        store.link_to(&sha512, &dest)?;
    }
    if index.map_to_resources {
        assets::reconstruct_assets(&index, &paths.asset_objects(), &paths.resources())?;
    }
    if index.virtual_assets {
        assets::reconstruct_assets(
            &index,
            &paths.asset_objects(),
            &paths.virtual_assets(index_id),
        )?;
    }
    Ok(())
}

fn client_sha1(version: &VersionInfo) -> Result<String> {
    version
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.client.as_ref())
        .and_then(|artifact| artifact.sha1.clone())
        .filter(|sha1| !sha1.is_empty())
        .ok_or_else(|| Error::MissingClient {
            id: version.id.clone(),
        })
}

fn dedup(requests: Vec<BlobRequest>) -> Vec<BlobRequest> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for request in requests {
        let key = request
            .sha1
            .clone()
            .filter(|sha1| !sha1.is_empty())
            .unwrap_or_else(|| request.url.clone());
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        out.push(request);
    }
    out
}

fn read_zip_file(path: &std::path::Path, name: &str) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|err| Error::io(path, err))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|err| Error::message(format!("could not read {}: {err}", path.display())))?;
    let mut entry = archive
        .by_name(name)
        .map_err(|_| Error::message(format!("{} is missing {name}", path.display())))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes)
        .map_err(|err| Error::message(format!("could not read {name}: {err}")))?;
    Ok(bytes)
}
