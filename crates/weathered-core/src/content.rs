//! Download vanilla versions and loader profiles into the shared store.
//!
//! Nothing is written into an instance here. Instance creation copies what it
//! needs out of the store afterwards.

use serde_json::Value;

use crate::assets::{self, parse_asset_index};
use crate::cancel::CancelFlag;
use crate::catalog::{self, CatalogItem};
use crate::download::{self, get_bytes_cancellable};
use crate::error::{Error, Result};
use crate::java::{self, JavaMode};
use crate::library::plan_libraries;
use crate::manifest;
use crate::paths::DataLayout;
use crate::platform::OsInfo;
use crate::processors::{self, Subst};
use crate::progress::ProgressSink;
use crate::rules::FeatureSet;
use crate::store::{BlobRequest, Store};
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
#[derive(Debug, Clone)]
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
    /// Raised by the UI to stop the download without waiting for HTTP timeouts.
    pub cancel: CancelFlag,
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
    let asset_count = queue_assets(
        client,
        layout,
        &store,
        &resolved.info,
        &mut requests,
        &request.cancel,
        progress,
    )
    .await?;
    let _stats = store
        .fetch_all(
            client,
            requests,
            request.concurrency,
            progress,
            &request.cancel,
        )
        .await?;
    request.cancel.check()?;
    if let Some(index_ref) = resolved.info.asset_index.clone() {
        if let Some(sha512) = store.lookup_sha1(&index_ref.sha1)? {
            emit_stage(progress, "assets", "linking assets");
            let index_path = store.object_path(&sha512)?;
            let index_bytes =
                std::fs::read(&index_path).map_err(|err| Error::io(&index_path, err))?;
            let store = store.clone();
            let layout = layout.clone();
            let index_id = index_ref.id.clone();
            let cancel = request.cancel.clone();
            tokio::task::spawn_blocking(move || {
                publish_assets(&store, &layout, &index_id, &index_bytes, &cancel)
            })
            .await
            .map_err(|err| Error::message(format!("could not link assets: {err}")))??;
        }
    }
    if request.java_mode != JavaMode::Skip {
        request.cancel.check()?;
        emit_stage(progress, "java", "resolving Java");
        let shared = layout.shared_assets();
        let java_result = java::resolve_java(
            client,
            &shared,
            java::JavaRequest {
                spec: resolved.info.java_version.as_ref(),
                explicit: None,
                mode: request.java_mode,
                os: request.os,
                concurrency: request.concurrency,
            },
            progress,
        );
        tokio::select! {
            biased;
            () = request.cancel.cancelled() => return Err(Error::message("cancelled")),
            result = java_result => {
                result?;
            }
        }
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
        version: request.version,
        loader_version: request.loader_version,
        os: request.os,
        concurrency: request.concurrency,
        java_mode: request.java_mode,
        cancel: request.cancel.clone(),
    };
    let vanilla = download_vanilla(client, layout, &vanilla_request, progress).await?;
    request.cancel.check()?;
    emit_stage(progress, "loader", "loader versions");
    let list_url = format!("{meta_root}/{}", vanilla.game_version);
    let list_bytes = get_bytes_cancellable(client, &list_url, &request.cancel).await?;
    let list: Value =
        serde_json::from_slice(&list_bytes).map_err(|err| Error::json("loader list", err))?;
    let selected = catalog::select_loader_version(&list, request.loader_version)?;
    request.cancel.check()?;
    emit_stage(progress, "loader", "loader profile");
    let profile_url = format!(
        "{meta_root}/{}/{selected}/profile/json",
        vanilla.game_version
    );
    let profile_bytes = get_bytes_cancellable(client, &profile_url, &request.cancel).await?;
    let profile = version::parse_version(&profile_bytes)?;
    let store = Store::open(layout.store_root())?;
    let stored_json = store.ingest(&profile_bytes, None, None, Some(&profile_url))?;
    let requests = version_blobs(&profile, request.os)?;
    store
        .fetch_all(
            client,
            requests,
            request.concurrency,
            progress,
            &request.cancel,
        )
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

/// Loader builds for the version picker.
pub async fn list_loader_builds(
    client: &reqwest::Client,
    loader: Loader,
    game: &str,
    cancel: &CancelFlag,
    progress: &ProgressSink,
) -> Result<Vec<catalog::LoaderBuild>> {
    cancel.check()?;
    emit_stage(progress, "loader", "loader versions");
    match loader {
        Loader::Fabric => {
            let url = format!("{FABRIC_META}/{game}");
            let bytes = get_bytes_cancellable(client, &url, cancel).await?;
            let value =
                serde_json::from_slice(&bytes).map_err(|err| Error::json("fabric loaders", err))?;
            Ok(catalog::parse_loader_builds(&value))
        }
        Loader::Quilt => {
            let url = format!("{QUILT_META}/{game}");
            let bytes = get_bytes_cancellable(client, &url, cancel).await?;
            let value =
                serde_json::from_slice(&bytes).map_err(|err| Error::json("quilt loaders", err))?;
            Ok(catalog::parse_loader_builds(&value))
        }
        Loader::Forge => {
            let promos = get_bytes_cancellable(
                client,
                "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
                cancel,
            )
            .await?;
            let meta =
                get_bytes_cancellable(client, &format!("{FORGE_MAVEN}/maven-metadata.xml"), cancel)
                    .await?;
            let promo_value = serde_json::from_slice(&promos)
                .map_err(|err| Error::json("forge promotions", err))?;
            let versions = catalog::maven_versions(&String::from_utf8_lossy(&meta));
            Ok(catalog::forge_builds_for_game(
                game,
                &versions,
                &promo_value,
            ))
        }
        Loader::NeoForge => {
            let meta = get_bytes_cancellable(
                client,
                &format!("{NEOFORGE_MAVEN}/maven-metadata.xml"),
                cancel,
            )
            .await?;
            let versions = catalog::maven_versions(&String::from_utf8_lossy(&meta));
            Ok(catalog::neoforge_builds_for_game(game, &versions))
        }
        Loader::Vanilla => Ok(Vec::new()),
    }
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
    let coordinate = request
        .loader_version
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .unwrap_or(request.version.trim());
    if coordinate.is_empty() {
        return Err(Error::message(format!(
            "pass a {kind} version, or a Minecraft version and a loader version"
        )));
    }
    request.cancel.check()?;
    emit_stage(progress, "loader", "installer");
    let url = format!("{maven}/{coordinate}/{artifact}-{coordinate}-installer.jar");
    let store = Store::open(layout.store_root())?;
    store
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
            &request.cancel,
        )
        .await?;
    let record = store
        .lookup_url(&url)?
        .ok_or_else(|| Error::message(format!("{kind} installer was not stored")))?;
    let installer_path = store.object_path(&record.sha512)?;
    let version_json = read_zip_file(&installer_path, "version.json")?;
    let profile_bytes = read_zip_file(&installer_path, "install_profile.json")?;
    let install = processors::parse_install_profile(&profile_bytes)?;
    if !install.supported {
        return Err(Error::message(processors::unsupported_message(kind)));
    }
    let version_info = version::parse_version(&version_json)?;
    let game_version = install
        .minecraft
        .clone()
        .or_else(|| version_info.inherits_from.clone())
        .or_else(|| forge_game_version(coordinate).map(str::to_string))
        .unwrap_or_else(|| request.version.to_string());
    let mut vanilla_request = request.clone();
    vanilla_request.loader = Loader::Vanilla;
    vanilla_request.version = &game_version;
    vanilla_request.loader_version = None;
    let vanilla = download_vanilla(client, layout, &vanilla_request, progress).await?;
    request.cancel.check()?;
    emit_stage(progress, "loader", "libraries");
    let mut requests = version_blobs(&version_info, request.os)?;
    let profile_version = version::VersionInfo {
        id: coordinate.to_string(),
        libraries: Some(install.libraries.clone()),
        ..empty_version()
    };
    requests.extend(version_blobs(&profile_version, request.os)?);
    store
        .fetch_all(
            client,
            requests,
            request.concurrency,
            progress,
            &request.cancel,
        )
        .await?;
    let libraries = layout.libraries();
    materialize_libraries(
        &store,
        &version_info,
        &install.libraries,
        request.os,
        &libraries,
    )?;
    let work = layout.root.join("store").join("work").join(kind);
    extract_embedded(&installer_path, &work)?;
    let client_jar = vanilla_client_path(layout, &vanilla)?;
    emit_stage(progress, "loader", "processors");
    let java = java::resolve_java(
        client,
        &layout.shared_assets(),
        java::JavaRequest {
            spec: version_info.java_version.as_ref(),
            explicit: None,
            mode: request.java_mode,
            os: request.os,
            concurrency: request.concurrency,
        },
        progress,
    )
    .await?;
    let libraries_owned = libraries.clone();
    let client_owned = client_jar.clone();
    let installer_owned = installer_path.clone();
    let work_owned = work.clone();
    let version_owned = game_version.clone();
    let data_owned = install.data.clone();
    let processors_owned = install.processors.clone();
    let java_path = java.0.path.clone();
    let cancel = request.cancel.clone();
    tokio::task::spawn_blocking(move || {
        let subst = Subst {
            libraries: &libraries_owned,
            minecraft_jar: &client_owned,
            installer: &installer_owned,
            extracted: &work_owned,
            minecraft_version: &version_owned,
            data: &data_owned,
        };
        let profile = processors::InstallProfile {
            minecraft: None,
            data: std::collections::BTreeMap::new(),
            processors: processors_owned,
            libraries: Vec::new(),
            supported: true,
        };
        processors::run_processors(&java_path, &profile, &subst, &cancel)
    })
    .await
    .map_err(|err| Error::message(format!("processor task failed: {err}")))??;
    ingest_library_tree(&store, &libraries)?;
    let stored_json = store.ingest(&version_json, None, None, None)?;
    let item = CatalogItem {
        id: format!("{kind}-{coordinate}"),
        kind: kind.into(),
        name: format!("{kind} {coordinate}"),
        game_version: game_version.clone(),
        loader_version: coordinate.to_string(),
        version_id: version_info.id,
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

fn empty_version() -> VersionInfo {
    serde_json::from_str(r#"{"id":"profile"}"#).expect("empty version")
}

fn forge_game_version(coordinate: &str) -> Option<&str> {
    let (game, build) = coordinate.rsplit_once('-')?;
    if build.is_empty() || !game.starts_with('1') {
        return None;
    }
    Some(game)
}

fn materialize_libraries(
    store: &Store,
    version: &VersionInfo,
    extra: &[crate::version::Library],
    os: &OsInfo,
    libraries: &std::path::Path,
) -> Result<()> {
    let mut libraries_list = version.libraries.clone().unwrap_or_default();
    libraries_list.extend(extra.iter().cloned());
    let plan = plan_libraries(&libraries_list, libraries, os, &FeatureSet::default())?;
    for file in plan.downloads {
        if file.dest.is_file() {
            continue;
        }
        let sha512 = if let Some(sha1) = &file.sha1 {
            store.lookup_sha1(sha1)?
        } else {
            None
        };
        let sha512 = match sha512 {
            Some(hash) => Some(hash),
            None if !file.url.is_empty() => {
                store.lookup_url(&file.url)?.map(|record| record.sha512)
            }
            None => None,
        };
        if let Some(hash) = sha512 {
            store.link_to(&hash, &file.dest)?;
        }
    }
    Ok(())
}

fn vanilla_client_path(layout: &DataLayout, vanilla: &CatalogItem) -> Result<std::path::PathBuf> {
    let store = Store::open(layout.store_root())?;
    let sha512 = store
        .lookup_sha1(&vanilla.client_sha1)?
        .ok_or_else(|| Error::message("the vanilla client jar is not in the store"))?;
    store.object_path(&sha512)
}

fn extract_embedded(installer: &std::path::Path, work: &std::path::Path) -> Result<()> {
    let file = std::fs::File::open(installer).map_err(|err| Error::io(installer, err))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|err| Error::message(format!("could not read installer: {err}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| Error::message(format!("could not read installer entry: {err}")))?;
        let name = entry
            .name()
            .map_err(|err| Error::message(format!("installer entry name: {err}")))?;
        if !name.starts_with("data/") || entry.is_dir() {
            continue;
        }
        let dest = work.join(name.as_ref());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Error::io(parent, err))?;
        }
        let mut output = std::fs::File::create(&dest).map_err(|err| Error::io(&dest, err))?;
        std::io::copy(&mut entry, &mut output).map_err(|err| Error::io(&dest, err))?;
    }
    Ok(())
}

fn ingest_library_tree(store: &Store, root: &std::path::Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|err| Error::io(&path, err))?;
            store.ingest(&bytes, None, None, None)?;
        }
    }
    Ok(())
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
            sha512: file.sha512,
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
    cancel: &CancelFlag,
    progress: &ProgressSink,
) -> Result<usize> {
    let Some(index_ref) = version.asset_index.as_ref() else {
        return Ok(0);
    };
    emit_stage(progress, "assets", "asset index");
    let bytes = get_bytes_cancellable(client, &index_ref.url, cancel).await?;
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
    cancel: &CancelFlag,
) -> Result<()> {
    cancel.check()?;
    let paths = layout.shared_assets();
    let index_path = paths.asset_index(index_id);
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| Error::io(parent, err))?;
    }
    std::fs::write(&index_path, index_bytes).map_err(|err| Error::io(&index_path, err))?;
    let index = parse_asset_index(index_bytes)?;
    for object in index.objects.values() {
        cancel.check()?;
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

fn emit_stage(progress: &ProgressSink, stage: &str, detail: &str) {
    progress.emit(crate::progress::ProgressEvent {
        stage: stage.to_string(),
        completed: 0,
        total: 0,
        detail: detail.to_string(),
    });
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
