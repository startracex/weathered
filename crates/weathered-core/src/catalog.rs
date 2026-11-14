//! Record of profiles that have been downloaded into the shared store.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoResultExt, Result};
use crate::paths::{DataLayout, validate_version_id};

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

/// One loader build the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderBuild {
    /// Version passed to the downloader. Forge uses `1.21.1-52.0.28`.
    pub version: String,
    /// Stable or otherwise suitable to show before the beta toggle.
    pub stable: bool,
    /// Preferred build for this game version.
    pub recommended: bool,
}

/// Fabric or Quilt loader list. Newest stable entry is marked recommended.
pub fn parse_loader_builds(document: &serde_json::Value) -> Vec<LoaderBuild> {
    let mut builds = Vec::new();
    for node in loader_nodes(document) {
        let Some(version) = node.get("version").and_then(|value| value.as_str()) else {
            continue;
        };
        if version.is_empty() {
            continue;
        }
        let stable = node
            .get("stable")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        builds.push(LoaderBuild {
            version: version.to_string(),
            stable,
            recommended: false,
        });
    }
    if let Some(index) = builds.iter().position(|build| build.stable) {
        builds[index].recommended = true;
    } else if let Some(first) = builds.first_mut() {
        first.recommended = true;
    }
    builds
}

/// Forge builds for one Minecraft version.
///
/// `promos` is `promotions_slim.json`. Recommended, then latest, are stable.
/// Older builds stay hidden until the beta toggle is on.
pub fn forge_builds_for_game(
    game: &str,
    versions: &[String],
    promos: &serde_json::Value,
) -> Vec<LoaderBuild> {
    let promo_map = promos.get("promos").unwrap_or(promos);
    let recommended = promo_map
        .get(format!("{game}-recommended"))
        .and_then(|value| value.as_str())
        .map(|build| format!("{game}-{build}"));
    let latest = promo_map
        .get(format!("{game}-latest"))
        .and_then(|value| value.as_str())
        .map(|build| format!("{game}-{build}"));
    let mut builds = Vec::new();
    for version in versions {
        let Some(rest) = version.strip_prefix(game) else {
            continue;
        };
        if !rest.starts_with('-') {
            continue;
        }
        let is_recommended = recommended.as_deref() == Some(version.as_str());
        let is_latest = latest.as_deref() == Some(version.as_str());
        builds.push(LoaderBuild {
            version: version.clone(),
            stable: is_recommended || is_latest,
            recommended: is_recommended || (recommended.is_none() && is_latest),
        });
    }
    if !builds.iter().any(|build| build.recommended)
        && let Some(first) = builds.first_mut()
    {
        first.recommended = true;
        first.stable = true;
    }
    builds
}

/// NeoForge builds whose version number matches a Minecraft release.
///
/// `1.21.1` matches `21.1.*`. `1.20.1` also matches the older `47.1.*` line.
/// The newest match is recommended. Names containing `beta` or `alpha` are unstable.
pub fn neoforge_builds_for_game(game: &str, versions: &[String]) -> Vec<LoaderBuild> {
    let prefixes = neoforge_prefixes(game);
    let mut builds: Vec<LoaderBuild> = versions
        .iter()
        .filter(|version| prefixes.iter().any(|prefix| version.starts_with(prefix)))
        .map(|version| {
            let lower = version.to_ascii_lowercase();
            let unstable = lower.contains("beta") || lower.contains("alpha");
            LoaderBuild {
                version: version.clone(),
                stable: !unstable,
                recommended: false,
            }
        })
        .collect();
    builds.reverse();
    if let Some(best) = builds.iter().position(|build| build.stable) {
        builds[best].recommended = true;
    } else if let Some(first) = builds.first_mut() {
        first.recommended = true;
    }
    builds
}

/// Maven prefixes NeoForge uses for a Minecraft version.
pub fn neoforge_prefixes(game: &str) -> Vec<String> {
    let parts: Vec<&str> = game.split('.').collect();
    let mut prefixes = Vec::new();
    if parts.len() >= 3 {
        prefixes.push(format!("{}.{}.", parts[1], parts[2]));
    } else if parts.len() == 2 {
        prefixes.push(format!("{}.0.", parts[1]));
    }
    if game == "1.20.1" {
        prefixes.push("47.1.".into());
    }
    prefixes
}

/// Builds shown before or after the beta toggle.
pub fn visible_builds(builds: &[LoaderBuild], show_unstable: bool) -> Vec<LoaderBuild> {
    builds
        .iter()
        .filter(|build| show_unstable || build.stable)
        .cloned()
        .collect()
}

/// Index of the recommended build, or the first row.
pub fn preferred_index(builds: &[LoaderBuild]) -> usize {
    builds
        .iter()
        .position(|build| build.recommended)
        .unwrap_or(0)
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

    #[test]
    fn loader_list_marks_the_newest_stable_build() {
        let builds = parse_loader_builds(&serde_json::json!([
            { "loader": { "version": "0.20.1", "stable": false } },
            { "loader": { "version": "0.19.0", "stable": true } },
            { "loader": { "version": "0.18.0", "stable": true } }
        ]));
        assert_eq!(builds[1].version, "0.19.0");
        assert!(builds[1].recommended);
        let shown = visible_builds(&builds, false);
        assert_eq!(shown.len(), 2);
        assert_eq!(preferred_index(&shown), 0);
    }

    #[test]
    fn forge_and_neoforge_lists_follow_the_game_version() {
        let promos = serde_json::json!({
            "promos": { "1.21.1-recommended": "52.0.1", "1.21.1-latest": "52.0.9" }
        });
        let versions = vec![
            "1.20.1-47.3.0".into(),
            "1.21.1-52.0.1".into(),
            "1.21.1-52.0.9".into(),
            "1.21.1-51.0.0".into(),
        ];
        let builds = forge_builds_for_game("1.21.1", &versions, &promos);
        assert_eq!(builds.len(), 3);
        assert!(
            builds
                .iter()
                .any(|build| build.version == "1.21.1-52.0.1" && build.recommended)
        );
        assert_eq!(visible_builds(&builds, false).len(), 2);
        let neo = neoforge_builds_for_game(
            "1.21.1",
            &[
                "21.1.80".into(),
                "21.1.81-beta".into(),
                "21.0.1".into(),
                "47.1.100".into(),
            ],
        );
        assert_eq!(neo[0].version, "21.1.81-beta");
        assert!(!neo[0].stable);
        assert!(neo[1].recommended);
        assert!(neoforge_builds_for_game("1.20.1", &["47.1.100".into()])[0].recommended);
        assert!(
            builds
                .iter()
                .all(|build| build.version.starts_with("1.21.1-"))
        );
        let older = parse_loader_builds(&serde_json::json!([
            { "loader": { "version": "0.14.0", "stable": true } }
        ]));
        let newer = parse_loader_builds(&serde_json::json!([
            { "loader": { "version": "0.20.1", "stable": true } }
        ]));
        assert_eq!(older[0].version, "0.14.0");
        assert!(older[0].recommended);
        assert_eq!(newer[0].version, "0.20.1");
        assert!(newer[0].recommended);
    }
}
