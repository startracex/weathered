//! Asset indexes and the object store.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::download::RemoteFile;
use crate::error::{Error, IoResultExt, Result};

const OBJECT_BASE: &str = "https://resources.download.minecraft.net";

/// An asset index document.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    /// Logical path to object hash.
    #[serde(default)]
    pub objects: BTreeMap<String, AssetObject>,
    /// When true, files are also copied to `assets/virtual/<index>/`.
    #[serde(rename = "virtual", default)]
    pub virtual_assets: bool,
    /// When true, files are also copied to `resources/`.
    #[serde(default)]
    pub map_to_resources: bool,
}

/// One object in an asset index.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    /// SHA-1, which is also the object id.
    pub hash: String,
    /// Size in bytes.
    pub size: u64,
}

/// Parse an asset index.
pub fn parse_asset_index(bytes: &[u8]) -> Result<AssetIndex> {
    serde_json::from_slice(bytes).map_err(|err| Error::json("asset index", err))
}

/// Relative object path `<hash[0..2]>/<hash>`.
pub fn object_rel_path(hash: &str) -> Result<String> {
    if hash.len() < 2 {
        return Err(Error::message(format!("asset hash '{hash}' is too short")));
    }
    Ok(format!("{}/{}", &hash[..2], hash))
}

/// Downloads for every unique object in the index.
pub fn plan_asset_downloads(index: &AssetIndex, objects_root: &Path) -> Result<Vec<RemoteFile>> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for object in index.objects.values() {
        if !seen.insert(object.hash.clone()) {
            continue;
        }
        let rel = object_rel_path(&object.hash)?;
        let dest = objects_root.join(&object.hash[..2]).join(&object.hash);
        files.push(RemoteFile {
            url: format!("{OBJECT_BASE}/{rel}"),
            dest,
            sha1: Some(object.hash.clone()),
            size: Some(object.size),
            executable: false,
        });
    }
    Ok(files)
}

/// Recreate logical asset paths by hard-linking, or copying if that fails.
///
/// Used for `map_to_resources` and `virtual` indexes. Modern indexes are read
/// from the object store directly and do not need this step.
pub fn reconstruct_assets(
    index: &AssetIndex,
    objects_root: &Path,
    dest_root: &Path,
) -> Result<usize> {
    let mut linked = 0usize;
    for (logical, object) in &index.objects {
        let Some(relative) = safe_logical(logical) else {
            continue;
        };
        if object.hash.len() < 2 {
            return Err(Error::message(format!(
                "asset hash '{}' is too short",
                object.hash
            )));
        }
        let source = objects_root.join(&object.hash[..2]).join(&object.hash);
        let dest = dest_root.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        if dest.is_file() {
            linked += 1;
            continue;
        }
        if std::fs::hard_link(&source, &dest).is_err() {
            std::fs::copy(&source, &dest).io_at(&dest)?;
        }
        linked += 1;
    }
    Ok(linked)
}

fn safe_logical(logical: &str) -> Option<PathBuf> {
    if logical.is_empty() {
        return None;
    }
    let path = PathBuf::from(logical);
    if path.is_absolute() {
        return None;
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_and_legacy_flags() {
        let index = parse_asset_index(
            br#"{
              "map_to_resources": true,
              "objects": {
                "icons/icon_16x16.png": { "hash": "bdf48ef6b5d0d23bbb02e17d04865216179f510a", "size": 3665 },
                "READ_ME_I_AM_VERY_IMPORTANT": { "hash": "0d000710b71ca9aafabd8f587768431d0b560b32", "size": 546 }
              }
            }"#,
        )
        .unwrap();
        assert!(index.map_to_resources);
        assert!(!index.virtual_assets);
        assert_eq!(index.objects.len(), 2);
        assert_eq!(
            object_rel_path("bdf48ef6b5d0d23bbb02e17d04865216179f510a").unwrap(),
            "bd/bdf48ef6b5d0d23bbb02e17d04865216179f510a"
        );
        let files = plan_asset_downloads(&index, Path::new("objects")).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|file| file
            .url
            .ends_with("bd/bdf48ef6b5d0d23bbb02e17d04865216179f510a")));
        assert!(object_rel_path("a").is_err());
    }
}
