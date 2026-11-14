//! On-disk layout of a Minecraft game directory.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Launcher data directory: shared store, instance folders, and legacy files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    /// Root previously used as the single game directory.
    pub root: PathBuf,
}

impl DataLayout {
    /// Use `root` as the launcher data directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$WEATHERED_GAME_DIR`, or the platform data directory plus `weathered`.
    pub fn from_environment() -> Self {
        Self::new(GamePaths::default_root())
    }

    /// Content-addressed file store.
    pub fn store_root(&self) -> PathBuf {
        self.root.join("store")
    }

    /// One JSON file per downloaded profile.
    pub fn catalog_dir(&self) -> PathBuf {
        self.root.join("catalog")
    }

    /// Instance metadata and isolated game directories.
    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    /// `instances/<id>/`.
    pub fn instance_dir(&self, id: &str) -> PathBuf {
        self.instances_dir().join(id)
    }

    /// Isolated game directory for a new instance.
    pub fn instance_game(&self, id: &str) -> PathBuf {
        self.instance_dir(id).join("game")
    }

    /// Version JSON cache used while resolving Mojang documents.
    pub fn meta(&self) -> GamePaths {
        GamePaths::new(self.root.join("meta"))
    }

    /// Shared asset tree Minecraft reads (`assets/objects/<sha1>`).
    ///
    /// The bytes live once in the content store. This directory hard-links
    /// them under the SHA-1 names the game expects.
    pub fn shared_assets(&self) -> GamePaths {
        GamePaths::new(self.root.clone())
    }
}

/// Paths inside one game directory.
///
/// The layout matches the official client: `versions/`, `libraries/`, and
/// `assets/`. Launcher-only files live under `weathered/` so they are not mixed
/// into folders the game reads.
#[derive(Debug, Clone)]
pub struct GamePaths {
    /// Root of the game directory.
    pub root: PathBuf,
}

impl GamePaths {
    /// Use `root` as the game directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default directory: `$WEATHERED_GAME_DIR`, or the platform data directory
    /// plus `weathered`.
    pub fn default_root() -> PathBuf {
        if let Some(dir) = std::env::var_os("WEATHERED_GAME_DIR") {
            return PathBuf::from(dir);
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("weathered")
    }

    /// `versions/`.
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// `versions/<id>/`.
    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }

    /// `versions/<id>/<id>.json`.
    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }

    /// `versions/<id>/<id>.jar`.
    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }

    /// Directory extracted native libraries are written to.
    pub fn natives_dir(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("natives")
    }

    /// `libraries/`.
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    /// `assets/`.
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    /// `assets/indexes/`.
    pub fn asset_indexes(&self) -> PathBuf {
        self.assets().join("indexes")
    }

    /// `assets/indexes/<id>.json`.
    pub fn asset_index(&self, id: &str) -> PathBuf {
        self.asset_indexes().join(format!("{id}.json"))
    }

    /// `assets/objects/`.
    pub fn asset_objects(&self) -> PathBuf {
        self.assets().join("objects")
    }

    /// `assets/objects/<hash[0..2]>/<hash>`.
    pub fn asset_object(&self, hash: &str) -> PathBuf {
        let prefix = hash.get(..2).unwrap_or("00");
        self.asset_objects().join(prefix).join(hash)
    }

    /// `resources/`, used by versions whose asset index sets `map_to_resources`.
    pub fn resources(&self) -> PathBuf {
        self.root.join("resources")
    }

    /// `assets/virtual/<index>/`, used by indexes that set `virtual`.
    pub fn virtual_assets(&self, index_id: &str) -> PathBuf {
        self.assets().join("virtual").join(index_id)
    }

    /// `assets/log_configs/`.
    pub fn log_configs(&self) -> PathBuf {
        self.assets().join("log_configs")
    }

    /// `assets/log_configs/<id>`.
    pub fn log_config(&self, id: &str) -> PathBuf {
        self.log_configs().join(id)
    }

    /// `runtime/`.
    pub fn runtime_root(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// `runtime/<component>/<platform>/`.
    pub fn runtime_dir(&self, component: &str, platform: &str) -> PathBuf {
        self.runtime_root().join(component).join(platform)
    }

    /// Launcher metadata that the game itself does not read.
    pub fn metadata_dir(&self) -> PathBuf {
        self.root.join("weathered")
    }

    /// Cached copy of Mojang's version manifest.
    pub fn manifest_cache(&self) -> PathBuf {
        self.metadata_dir().join("version_manifest_v2.json")
    }

    /// Java argument file used when the classpath is very long.
    pub fn argfile(&self) -> PathBuf {
        self.metadata_dir().join("launch-args.txt")
    }
}

/// Reject version ids that could escape the versions directory.
pub fn validate_version_id(id: &str) -> Result<()> {
    let bad = id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains('\0')
        || id.contains("..")
        || id.chars().any(|ch| ch.is_whitespace() || ch.is_control());
    if bad {
        return Err(Error::message(format!("invalid version id '{id}'")));
    }
    Ok(())
}

/// Turn a Mojang relative path into a [`PathBuf`], rejecting `..` segments.
pub fn safe_rel_path(path: &str) -> Result<PathBuf> {
    if path.is_empty() {
        return Err(Error::message("empty relative path"));
    }
    let mut out = PathBuf::new();
    for part in path.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(Error::message(format!(
                "relative path '{path}' escapes its directory"
            )));
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        return Err(Error::message(format!("relative path '{path}' is empty")));
    }
    Ok(out)
}

/// Display a relative path with forward slashes, for URLs and tests.
pub fn portable(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ids_reject_path_traversal() {
        assert!(validate_version_id("1.21.1").is_ok());
        assert!(validate_version_id("26.4-snapshot-8").is_ok());
        assert!(validate_version_id("../secrets").is_err());
        assert!(validate_version_id("a/b").is_err());
        assert!(validate_version_id("").is_err());
    }

    #[test]
    fn safe_rel_path_keeps_maven_segments() {
        let path = safe_rel_path("com/mojang/brigadier/1.0.17/brigadier-1.0.17.jar").unwrap();
        assert_eq!(
            portable(&path),
            "com/mojang/brigadier/1.0.17/brigadier-1.0.17.jar"
        );
        assert!(safe_rel_path("com/../../etc/passwd").is_err());
    }
}
