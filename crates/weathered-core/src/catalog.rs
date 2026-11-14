//! Record of profiles that have been downloaded into the shared store.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoResultExt, Result};
use crate::paths::{validate_version_id, DataLayout};

/// One downloaded game or loader profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogItem {
    /// Stable id, such as `vanilla-1.21.1` or `fabric-1.21.1-0.16.9`.
    pub id: String,
    /// `vanilla`, `fabric`, `quilt`, `forge`, or `neoforge`.
    pub kind: String,
    /// Label shown in the UI.
    pub name: String,
    /// Minecraft version this profile runs.
    pub game_version: String,
    /// Loader version. Empty for vanilla.
    pub loader_version: String,
    /// Version id of the JSON that should be launched.
    pub version_id: String,
    /// SHA-512 of that version JSON in the store.
    pub version_json_sha512: String,
    /// SHA-512 of the parent JSON, when the profile uses `inheritsFrom`.
    #[serde(default)]
    pub parent_json_sha512: String,
    /// Parent version id. Empty when there is no parent.
    #[serde(default)]
    pub parent_id: String,
    /// SHA-1 of the client jar.
    pub client_sha1: String,
    /// False when the profile is stored but cannot be launched yet.
    pub launchable: bool,
    /// Why a profile cannot be launched, or an empty string.
    #[serde(default)]
    pub note: String,
}

impl CatalogItem {
    /// Path of this item's JSON file.
    pub fn path(layout: &DataLayout, id: &str) -> Result<PathBuf> {
        validate_version_id(id)?;
        Ok(layout.catalog_dir().join(format!("{id}.json")))
    }

    /// Write this item into the catalog directory.
    pub fn save(&self, layout: &DataLayout) -> Result<()> {
        let path = Self::path(layout, &self.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|err| Error::json("catalog item", err))?;
        std::fs::write(&path, text).io_at(&path)
    }

    /// Load one catalog item.
    pub fn load(layout: &DataLayout, id: &str) -> Result<Self> {
        let path = Self::path(layout, id)?;
        let bytes = std::fs::read(&path).io_at(&path)?;
        serde_json::from_slice(&bytes).map_err(|err| Error::json(format!("catalog {id}"), err))
    }
}

/// Every catalog item, sorted by id.
pub fn list(layout: &DataLayout) -> Result<Vec<CatalogItem>> {
    let dir = layout.catalog_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).io_at(&path)?;
        let item: CatalogItem = serde_json::from_slice(&bytes)
            .map_err(|err| Error::json(format!("catalog {}", path.display()), err))?;
        items.push(item);
    }
    items.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(items)
}

/// Find a catalog item by id or by kind plus game version.
pub fn find(layout: &DataLayout, query: &str) -> Result<Option<CatalogItem>> {
    let items = list(layout)?;
    if let Some(item) = items.iter().find(|item| item.id == query) {
        return Ok(Some(item.clone()));
    }
    let matches: Vec<_> = items
        .into_iter()
        .filter(|item| item.game_version == query || item.version_id == query || item.name == query)
        .collect();
    if matches.len() == 1 {
        return Ok(matches.into_iter().next());
    }
    Ok(matches.into_iter().find(|item| item.kind == "vanilla"))
}

/// Loader version to download.
///
/// `document` is a Fabric or Quilt metadata response. An explicit `requested`
/// version is used as-is. Otherwise the newest stable entry wins, then the
/// first entry.
pub fn select_loader_version(
    document: &serde_json::Value,
    requested: Option<&str>,
) -> Result<String> {
    if let Some(version) = requested
        .map(str::trim)
        .filter(|version| !version.is_empty())
    {
        return Ok(version.to_string());
    }
    let mut stable = None;
    let mut first = None;
    for node in loader_nodes(document) {
        let Some(version) = node.get("version").and_then(|value| value.as_str()) else {
            continue;
        };
        if first.is_none() {
            first = Some(version.to_string());
        }
        if node.get("stable").and_then(|value| value.as_bool()) == Some(true) && stable.is_none() {
            stable = Some(version.to_string());
        }
    }
    stable
        .or(first)
        .ok_or_else(|| Error::message("the loader metadata did not include a version"))
}

fn loader_nodes(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| item.get("loader").unwrap_or(item))
            .collect(),
        serde_json::Value::Object(_) => value
            .get("loader")
            .map(|loader| vec![loader])
            .unwrap_or_else(|| vec![value]),
        _ => Vec::new(),
    }
}

/// Version strings from a Maven `maven-metadata.xml` document.
pub fn maven_versions(xml: &str) -> Vec<String> {
    let mut versions = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        rest = &rest[start + "<version>".len()..];
        let Some(end) = rest.find("</version>") else {
            break;
        };
        let version = rest[..end].trim();
        if !version.is_empty() && !version.contains('<') {
            versions.push(version.to_string());
        }
        rest = &rest[end + "</version>".len()..];
    }
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_loader_is_preferred() {
        let document = serde_json::json!([
            { "loader": { "version": "0.16.10", "stable": false } },
            { "loader": { "version": "0.16.9", "stable": true } }
        ]);
        assert_eq!(select_loader_version(&document, None).unwrap(), "0.16.9");
        assert_eq!(
            select_loader_version(&document, Some("0.16.10")).unwrap(),
            "0.16.10"
        );
    }

    #[test]
    fn maven_metadata_lists_versions() {
        let xml = r#"
            <metadata>
              <versioning>
                <latest>1.21.1-52.0.1</latest>
                <versions>
                  <version>1.20.1-47.3.0</version>
                  <version>1.21.1-52.0.1</version>
                </versions>
              </versioning>
            </metadata>
        "#;
        assert_eq!(
            maven_versions(xml),
            vec!["1.20.1-47.3.0".to_string(), "1.21.1-52.0.1".to_string()]
        );
    }
}
