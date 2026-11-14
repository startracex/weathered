//! Isolated instances.
//!
//! Each instance has its own game directory for mods, saves, config, and
//! resource packs. The client jar and version JSON are copied in from the
//! store. Libraries stay in the store and are passed on the classpath by
//! absolute path. Assets are hard-linked into the shared `assets/` tree,
//! because Minecraft looks them up by SHA-1 rather than by the store's SHA-512.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::CatalogItem;
use crate::content::{self, Loader};
use crate::error::{Error, IoResultExt, Result};
use crate::java::{self, JavaMode};
use crate::launch::{self, build_launch_plan, LaunchPlan, LaunchRequest, LaunchSpec};
use crate::library::{self, plan_libraries, NativeJar};
use crate::manifest::VersionManifest;
use crate::modpack::{self, MrPack};
use crate::paths::{validate_version_id, DataLayout, GamePaths};
use crate::platform::OsInfo;
use crate::progress::ProgressSink;
use crate::rules::FeatureSet;
use crate::store::{BlobRequest, Store};
use crate::version::{self, VersionInfo};
use crate::{LAUNCHER_NAME, LAUNCHER_VERSION};

/// Metadata stored in `instances/<id>/instance.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// Directory name under `instances/`.
    pub id: String,
    /// Name shown in the UI.
    pub name: String,
    /// Minecraft version.
    pub game_version: String,
    /// `vanilla`, `fabric`, `quilt`, `forge`, or `neoforge`.
    pub loader: String,
    /// Loader version. Empty for vanilla.
    pub loader_version: String,
    /// Version JSON id used to launch.
    pub version_id: String,
    /// `-Xmx` override. Empty uses the caller's memory setting.
    #[serde(default)]
    pub memory: String,
    /// Java binary override. Empty lets Weathered pick one.
    #[serde(default)]
    pub java: String,
    /// This instance points at a game directory created before the store existed.
    #[serde(default)]
    pub legacy: bool,
    /// Absolute game directory.
    pub game_dir: String,
}

impl Instance {
    fn path(layout: &DataLayout, id: &str) -> Result<PathBuf> {
        validate_version_id(id)?;
        Ok(layout.instance_dir(id).join("instance.toml"))
    }

    fn save(&self, layout: &DataLayout) -> Result<()> {
        let path = Self::path(layout, &self.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|err| Error::message(format!("could not encode instance: {err}")))?;
        std::fs::write(&path, text).io_at(&path)
    }

    fn load_at(layout: &DataLayout, id: &str) -> Result<Self> {
        let path = Self::path(layout, id)?;
        let text = std::fs::read_to_string(&path).io_at(&path)?;
        toml::from_str(&text)
            .map_err(|err| Error::message(format!("could not parse {}: {err}", path.display())))
    }
}

/// Every instance, sorted by name.
pub fn list(layout: &DataLayout) -> Result<Vec<Instance>> {
    let dir = layout.instances_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if validate_version_id(&id).is_err() {
            continue;
        }
        let path = entry.path().join("instance.toml");
        if !path.is_file() {
            continue;
        }
        items.push(Instance::load_at(layout, &id)?);
    }
    items.sort_by_key(|item| item.name.to_ascii_lowercase());
    Ok(items)
}

/// Find an instance by id or by name.
pub fn find(layout: &DataLayout, query: &str) -> Result<Option<Instance>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(None);
    }
    let items = list(layout)?;
    if let Some(item) = items.iter().find(|item| item.id == query) {
        return Ok(Some(item.clone()));
    }
    let named: Vec<_> = items
        .into_iter()
        .filter(|item| item.name.eq_ignore_ascii_case(query))
        .collect();
    match named.len() {
        0 => Ok(None),
        1 => Ok(named.into_iter().next()),
        _ => Err(Error::message(format!(
            "more than one instance is named '{query}'. Use the instance id."
        ))),
    }
}

/// Create one legacy instance per version already installed in the old layout.
///
/// Existing files are not moved. Later calls do nothing once any instance exists.
pub fn migrate_legacy(layout: &DataLayout) -> Result<Vec<Instance>> {
    if !list(layout)?.is_empty() {
        return Ok(Vec::new());
    }
    let mut created = Vec::new();
    for version in installed_versions(&layout.root) {
        let instance = Instance {
            id: unique_id(layout, &format!("legacy-{}", slug(&version.id)))?,
            name: version.id.clone(),
            game_version: version.id.clone(),
            loader: "vanilla".into(),
            loader_version: String::new(),
            version_id: version.id,
            memory: String::new(),
            java: String::new(),
            legacy: true,
            game_dir: layout.root.display().to_string(),
        };
        instance.save(layout)?;
        created.push(instance);
    }
    Ok(created)
}

/// Versions that have both a JSON document and a client jar in `root/versions`.
pub fn installed_versions(root: &Path) -> Vec<InstalledVersion> {
    let dir = root.join("versions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if validate_version_id(&id).is_err() {
            continue;
        }
        let json_path = entry.path().join(format!("{id}.json"));
        let jar_path = entry.path().join(format!("{id}.jar"));
        if json_path.is_file() && jar_path.is_file() {
            let kind = std::fs::read(&json_path)
                .ok()
                .and_then(|bytes| version::parse_version(&bytes).ok())
                .and_then(|info| info.version_type)
                .unwrap_or_else(|| "installed".into());
            found.push(InstalledVersion { id, kind });
        }
    }
    found.sort_by(|left, right| left.id.cmp(&right.id));
    found
}

/// A version directory left by the old single-directory layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledVersion {
    /// Version id.
    pub id: String,
    /// `type` from the version JSON, when it parses.
    pub kind: String,
}

/// Create an instance from a catalog item, copying the client jar and version JSON.
pub fn create(
    layout: &DataLayout,
    name: &str,
    catalog_id: &str,
    memory: &str,
    java: &str,
) -> Result<Instance> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::message("instance name cannot be empty"));
    }
    let item = CatalogItem::load(layout, catalog_id)?;
    if !item.launchable {
        return Err(Error::message(if item.note.is_empty() {
            format!("{catalog_id} cannot be launched")
        } else {
            item.note
        }));
    }
    let id = unique_id(layout, &slug(name))?;
    let game_dir = layout.instance_game(&id);
    std::fs::create_dir_all(&game_dir).io_at(&game_dir)?;
    for folder in ["mods", "saves", "config", "resourcepacks", "shaderpacks"] {
        let path = game_dir.join(folder);
        std::fs::create_dir_all(&path).io_at(&path)?;
    }
    materialize_version(layout, &item, &game_dir)?;
    let instance = Instance {
        id,
        name: name.to_string(),
        game_version: item.game_version,
        loader: item.kind,
        loader_version: item.loader_version,
        version_id: item.version_id,
        memory: memory.trim().to_string(),
        java: java.trim().to_string(),
        legacy: false,
        game_dir: game_dir.display().to_string(),
    };
    instance.save(layout)?;
    Ok(instance)
}

/// Delete an instance directory.
///
/// A legacy instance only removes its metadata. The old game directory stays.
pub fn delete(layout: &DataLayout, query: &str) -> Result<Instance> {
    let instance = find(layout, query)?
        .ok_or_else(|| Error::message(format!("no instance matches '{query}'")))?;
    let dir = layout.instance_dir(&instance.id);
    if instance.legacy {
        let meta = Instance::path(layout, &instance.id)?;
        if meta.is_file() {
            std::fs::remove_file(&meta).io_at(&meta)?;
        }
        let _ = std::fs::remove_dir(&dir);
    } else {
        std::fs::remove_dir_all(&dir).io_at(&dir)?;
    }
    Ok(instance)
}

/// Download anything the pack needs, then copy mods and overrides into a new instance.
pub async fn import_mrpack(
    client: &reqwest::Client,
    layout: &DataLayout,
    pack_path: &Path,
    name: Option<&str>,
    os: &OsInfo,
    concurrency: usize,
    progress: &ProgressSink,
) -> Result<Instance> {
    let bytes = std::fs::read(pack_path).map_err(|err| Error::io(pack_path, err))?;
    let pack = modpack::parse_mrpack(&bytes)?;
    if pack.loader == "forge" || pack.loader == "neoforge" {
        return Err(Error::message(
            "Forge and NeoForge modpacks are not imported yet. Their installers need a processor step that Weathered does not run. Fabric, Quilt, and vanilla .mrpack files are supported. CurseForge packs are not supported.",
        ));
    }
    let loader = Loader::parse(&pack.loader)?;
    let loader_version = if pack.loader_version.is_empty() {
        None
    } else {
        Some(pack.loader_version.as_str())
    };
    let item = content::download(
        client,
        layout,
        content::DownloadRequest {
            version: &pack.game_version,
            loader,
            loader_version,
            os,
            concurrency,
            java_mode: JavaMode::Auto,
        },
        progress,
    )
    .await?;
    let instance_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(pack.name.as_str());
    let instance = create(layout, instance_name, &item.id, "", "")?;
    copy_pack_files(
        client,
        layout,
        &pack,
        Path::new(&instance.game_dir),
        concurrency,
        progress,
    )
    .await?;
    modpack::extract_overrides(&bytes, Path::new(&instance.game_dir))?;
    Ok(instance)
}

/// Build the launch command for an instance.
pub async fn prepare_launch(
    client: &reqwest::Client,
    layout: &DataLayout,
    query: &str,
    mut request: LaunchRequest,
    progress: &ProgressSink,
) -> Result<LaunchPlan> {
    let instance = find(layout, query)?
        .ok_or_else(|| Error::message(format!("no instance matches '{query}'")))?;
    if !instance.memory.trim().is_empty() {
        request.memory = instance.memory.clone();
    }
    if request.java.is_none() && !instance.java.trim().is_empty() {
        request.java = Some(PathBuf::from(instance.java.trim()));
    }
    let game_dir = PathBuf::from(&instance.game_dir);
    if instance.legacy {
        request.version_id = instance.version_id;
        return launch::prepare_launch(client, &GamePaths::new(&game_dir), request, progress).await;
    }
    let paths = GamePaths::new(&game_dir);
    let manifest = empty_manifest();
    let resolved =
        version::load_resolved(client, &paths, &manifest, &instance.version_id, false).await?;
    let store = Store::open(layout.store_root())?;
    let (classpath, natives) =
        classpath_from_store(&store, &resolved.info, &request.os, &request.features)?;
    let client_jar = paths.version_jar(&resolved.jar_id);
    if !client_jar.is_file() {
        return Err(Error::message(format!(
            "instance {} is missing the client jar at {}",
            instance.name,
            client_jar.display()
        )));
    }
    let natives_dir = paths.natives_dir(&resolved.info.id);
    let natives_for_task = natives.clone();
    let natives_dest = natives_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        library::reset_dir(&natives_dest)?;
        for native in &natives_for_task {
            library::extract_native_jar(&native.jar, &natives_dest, &native.exclude)?;
        }
        Ok(())
    })
    .await
    .map_err(|err| Error::message(format!("could not extract natives: {err}")))??;

    if let Some(index_ref) = resolved.info.asset_index.clone() {
        if let Some(sha512) = store.lookup_sha1(&index_ref.sha1)? {
            let index_path = store.object_path(&sha512)?;
            let bytes = std::fs::read(&index_path).map_err(|err| Error::io(&index_path, err))?;
            content::publish_assets(&store, layout, &index_ref.id, &bytes)?;
        }
    }
    if let Some(logging) = resolved
        .info
        .logging
        .clone()
        .and_then(|logging| logging.client)
    {
        if let Some(sha512) = store.lookup_sha1(&logging.file.sha1)? {
            store.copy_to(&sha512, &paths.log_config(&logging.file.id))?;
        }
    }

    let data_paths = GamePaths::new(&layout.root);
    let (java_install, _) = java::resolve_java(
        client,
        &data_paths,
        java::JavaRequest {
            spec: resolved.info.java_version.as_ref(),
            explicit: request.java.as_deref(),
            mode: request.java_mode,
            os: &request.os,
            concurrency: request.concurrency,
        },
        progress,
    )
    .await?;
    let index_name = version::asset_index_name(&resolved.info).unwrap_or_default();
    let shared = layout.shared_assets();
    build_launch_plan(LaunchSpec {
        version: resolved.info.clone(),
        classpath,
        client_jar,
        natives_dir,
        game_dir: game_dir.clone(),
        assets_dir: shared.assets(),
        assets_index_name: index_name.clone(),
        game_assets: launch::game_assets_dir(&shared, &index_name),
        libraries_dir: layout.store_root(),
        log_argument: launch::logging_argument(&resolved.info, &paths),
        session: request.session,
        java: java_install,
        memory: request.memory,
        os: request.os,
        features: request.features,
        client_id: request.client_id,
        quick_play_path: request.quick_play_path,
        quick_play_singleplayer: request.quick_play_singleplayer,
        quick_play_multiplayer: request.quick_play_multiplayer,
        quick_play_realms: request.quick_play_realms,
        width: request.width,
        height: request.height,
        launcher_name: LAUNCHER_NAME.into(),
        launcher_version: LAUNCHER_VERSION.into(),
        argfile: paths.argfile(),
        java_warning: None,
    })
}

fn materialize_version(layout: &DataLayout, item: &CatalogItem, game_dir: &Path) -> Result<()> {
    let store = Store::open(layout.store_root())?;
    let paths = GamePaths::new(game_dir);
    let json_sha = store.object_path(&item.version_json_sha512)?;
    copy_file(&json_sha, &paths.version_json(&item.version_id))?;
    if !item.parent_id.is_empty() {
        let parent = store.object_path(&item.parent_json_sha512)?;
        copy_file(&parent, &paths.version_json(&item.parent_id))?;
    }
    let sha512 = store.lookup_sha1(&item.client_sha1)?.ok_or_else(|| {
        Error::message(format!(
            "client jar {} is not in the store",
            item.client_sha1
        ))
    })?;
    let jar_id = if item.parent_id.is_empty() {
        item.version_id.as_str()
    } else {
        item.parent_id.as_str()
    };
    store.copy_to(&sha512, &paths.version_jar(jar_id))?;
    Ok(())
}

fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).io_at(parent)?;
    }
    std::fs::copy(src, dest).map_err(|err| Error::io(dest, err))?;
    Ok(())
}

async fn copy_pack_files(
    client: &reqwest::Client,
    layout: &DataLayout,
    pack: &MrPack,
    game_dir: &Path,
    concurrency: usize,
    progress: &ProgressSink,
) -> Result<()> {
    let store = Store::open(layout.store_root())?;
    let mut requests = Vec::new();
    for file in &pack.files {
        let url = file.downloads.first().cloned().unwrap_or_default();
        requests.push(BlobRequest {
            url,
            sha1: file.sha1.clone(),
            sha512: file.sha512.clone(),
            size: file.size,
        });
    }
    store
        .fetch_all(client, requests, concurrency, progress)
        .await?;
    for file in &pack.files {
        let sha512 = locate_pack_file(&store, file)?;
        let dest = game_dir.join(crate::paths::safe_rel_path(&file.path)?);
        store.copy_to(&sha512, &dest)?;
    }
    Ok(())
}

fn locate_pack_file(store: &Store, file: &crate::modpack::MrFile) -> Result<String> {
    if let Some(sha512) = file.sha512.as_deref() {
        if store
            .object_path(sha512)
            .map(|path| path.is_file())
            .unwrap_or(false)
        {
            return Ok(sha512.to_ascii_lowercase());
        }
    }
    if let Some(sha1) = file.sha1.as_deref() {
        if let Some(found) = store.lookup_sha1(sha1)? {
            return Ok(found);
        }
    }
    for url in &file.downloads {
        if let Some(record) = store.lookup_url(url)? {
            return Ok(record.sha512);
        }
    }
    Err(Error::message(format!(
        "modpack file {} was not stored",
        file.path
    )))
}

fn classpath_from_store(
    store: &Store,
    version: &VersionInfo,
    os: &OsInfo,
    features: &FeatureSet,
) -> Result<(Vec<PathBuf>, Vec<NativeJar>)> {
    let libraries = version.libraries.clone().unwrap_or_default();
    let plan = plan_libraries(&libraries, Path::new("libraries"), os, features)?;
    let mut by_dest = HashMap::new();
    for file in &plan.downloads {
        by_dest.insert(file.dest.clone(), file);
    }
    let map_path = |path: &Path| -> Result<PathBuf> {
        let Some(file) = by_dest.get(path) else {
            return Err(Error::message(format!(
                "no download record for {}",
                path.display()
            )));
        };
        if let Some(sha1) = &file.sha1 {
            if let Some(sha512) = store.lookup_sha1(sha1)? {
                return store.object_path(&sha512);
            }
        }
        if let Some(record) = store.lookup_url(&file.url)? {
            return store.object_path(&record.sha512);
        }
        Err(Error::message(format!(
            "library {} is not in the store. Download the profile first.",
            path.display()
        )))
    };
    let mut classpath = Vec::new();
    for path in &plan.classpath {
        classpath.push(map_path(path)?);
    }
    let mut natives = Vec::new();
    for native in plan.natives {
        natives.push(NativeJar {
            jar: map_path(&native.jar)?,
            exclude: native.exclude,
        });
    }
    Ok((classpath, natives))
}

fn empty_manifest() -> VersionManifest {
    serde_json::from_str(r#"{"latest":{"release":"","snapshot":""},"versions":[]}"#)
        .expect("empty manifest")
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "instance".into()
    } else {
        out
    }
}

fn unique_id(layout: &DataLayout, base: &str) -> Result<String> {
    let base = if base.is_empty() { "instance" } else { base };
    if !layout.instance_dir(base).exists() {
        return Ok(base.to_string());
    }
    for suffix in 2..10_000 {
        let id = format!("{base}-{suffix}");
        if !layout.instance_dir(&id).exists() {
            return Ok(id);
        }
    }
    Err(Error::message(format!(
        "could not allocate an id for {base}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogItem;
    use crate::store::sha512_hex;

    fn layout() -> (PathBuf, DataLayout) {
        let root = std::env::temp_dir().join(format!(
            "weathered-instance-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        (root.clone(), DataLayout::new(root))
    }

    #[test]
    fn create_copies_the_client_jar_and_leaves_the_store_intact() {
        let (root, layout) = layout();
        let store = Store::open(layout.store_root()).unwrap();
        let client = b"client-jar-bytes";
        let client_sha1 = crate::download::sha1_hex(client);
        let stored_client = store
            .ingest(client, Some(&client_sha1), None, None)
            .unwrap();
        let json = format!(
            r#"{{"id":"1.2.3","type":"release","mainClass":"net.minecraft.client.main.Main","downloads":{{"client":{{"sha1":"{client_sha1}","size":16,"url":"https://example.test/client.jar"}}}}}}"#
        );
        let stored_json = store.ingest(json.as_bytes(), None, None, None).unwrap();
        let item = CatalogItem {
            id: "vanilla-1.2.3".into(),
            kind: "vanilla".into(),
            name: "1.2.3".into(),
            game_version: "1.2.3".into(),
            loader_version: String::new(),
            version_id: "1.2.3".into(),
            version_json_sha512: stored_json.sha512,
            parent_json_sha512: String::new(),
            parent_id: String::new(),
            client_sha1: client_sha1.clone(),
            launchable: true,
            note: String::new(),
        };
        item.save(&layout).unwrap();
        let instance = create(&layout, "Skyblock", "vanilla-1.2.3", "4G", "").unwrap();
        let jar = PathBuf::from(&instance.game_dir)
            .join("versions")
            .join("1.2.3")
            .join("1.2.3.jar");
        assert_eq!(std::fs::read(&jar).unwrap(), client);
        assert!(store.object_path(&stored_client.sha512).unwrap().is_file());
        assert_eq!(store.object_count(), 2);
        assert!(PathBuf::from(&instance.game_dir).join("mods").is_dir());
        assert!(PathBuf::from(&instance.game_dir).join("saves").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&jar).unwrap().nlink(), 1);
        }
        let again = store
            .ingest(client, Some(&client_sha1), None, None)
            .unwrap();
        assert!(again.cached);
        assert_eq!(sha512_hex(client), stored_client.sha512);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_install_becomes_an_instance_without_moving_files() {
        let (root, layout) = layout();
        let version = root.join("versions").join("1.2.3");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("1.2.3.json"), br#"{"id":"1.2.3"}"#).unwrap();
        std::fs::write(version.join("1.2.3.jar"), b"jar").unwrap();
        let created = migrate_legacy(&layout).unwrap();
        assert_eq!(created.len(), 1);
        assert!(created[0].legacy);
        assert_eq!(created[0].version_id, "1.2.3");
        assert!(version.join("1.2.3.jar").is_file());
        assert!(migrate_legacy(&layout).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
