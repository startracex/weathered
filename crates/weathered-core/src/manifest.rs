//! Mojang's version manifest (`version_manifest_v2.json`).

use std::path::Path;

use serde::Deserialize;

use crate::download::{self, get_bytes};
use crate::error::{Error, IoResultExt, Result};

const MANIFEST_URLS: &[&str] = &[
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
];

/// The current release and snapshot named by the manifest.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LatestVersions {
    /// Newest release id.
    pub release: String,
    /// Newest snapshot id.
    pub snapshot: String,
}

/// One entry in the version list.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    /// Version id, such as `1.21.1` or `26.4-snapshot-8`.
    pub id: String,
    /// `release`, `snapshot`, `old_beta`, or `old_alpha`.
    #[serde(rename = "type")]
    pub version_type: String,
    /// URL of this version's JSON document.
    pub url: String,
    /// When this manifest entry was updated.
    pub time: Option<String>,
    /// When the version was published.
    pub release_time: Option<String>,
    /// SHA-1 of the version JSON, when the manifest provides one.
    pub sha1: Option<String>,
    /// Mojang compliance level.
    pub compliance_level: Option<i32>,
}

/// A fetched version manifest.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VersionManifest {
    /// Pointers to the newest release and snapshot.
    pub latest: LatestVersions,
    /// Every known version, newest first.
    pub versions: Vec<VersionEntry>,
}

/// Manifest plus whether it was read from the local cache.
#[derive(Debug, Clone)]
pub struct ManifestFetch {
    /// Parsed manifest.
    pub manifest: VersionManifest,
    /// `true` when the network request failed and the cache was used.
    pub from_cache: bool,
}

/// Kinds of versions a listing can filter on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionKind {
    /// Stable releases.
    Release,
    /// Snapshots and weekly development builds.
    Snapshot,
    /// Old beta versions.
    OldBeta,
    /// Old alpha versions.
    OldAlpha,
}

impl VersionKind {
    /// Value stored in the manifest's `type` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Snapshot => "snapshot",
            Self::OldBeta => "old_beta",
            Self::OldAlpha => "old_alpha",
        }
    }
}

impl VersionManifest {
    /// Find a version by exact id.
    pub fn find(&self, id: &str) -> Option<&VersionEntry> {
        self.versions.iter().find(|entry| entry.id == id)
    }

    /// Resolve `release` and `snapshot` aliases to the manifest's latest ids.
    pub fn resolve_id<'a>(&'a self, id: &'a str) -> &'a str {
        match id {
            "release" | "latest" | "latest-release" => self.latest.release.as_str(),
            "snapshot" | "latest-snapshot" => self.latest.snapshot.as_str(),
            other => other,
        }
    }

    /// Filter versions by kind and a case-insensitive id substring.
    ///
    /// `kinds == None` keeps every type. The manifest's order (newest first)
    /// is preserved.
    pub fn query<'a>(
        &'a self,
        kinds: Option<&[VersionKind]>,
        search: Option<&str>,
    ) -> Vec<&'a VersionEntry> {
        let needle = search.map(|value| value.to_ascii_lowercase());
        self.versions
            .iter()
            .filter(|entry| {
                if let Some(kinds) = kinds
                    && !kinds.iter().any(|kind| entry.version_type == kind.as_str())
                {
                    return false;
                }
                if let Some(needle) = &needle
                    && !entry.id.to_ascii_lowercase().contains(needle)
                {
                    return false;
                }
                true
            })
            .collect()
    }
}

/// Parse a version manifest document.
pub fn parse_manifest(bytes: &[u8]) -> Result<VersionManifest> {
    serde_json::from_slice(bytes).map_err(|err| Error::json("version manifest", err))
}

/// Download the manifest, falling back to `cache` if every URL fails.
pub async fn fetch_manifest(client: &reqwest::Client, cache: &Path) -> Result<ManifestFetch> {
    let mut last_error = None;
    for url in MANIFEST_URLS {
        match get_bytes(client, url).await {
            Ok(bytes) => {
                let manifest = parse_manifest(&bytes)?;
                if let Err(err) = download::write_atomic(cache, &bytes).await {
                    // A cache write should not hide a manifest that was just fetched.
                    let _ = err;
                }
                return Ok(ManifestFetch {
                    manifest,
                    from_cache: false,
                });
            }
            Err(err) => last_error = Some(err),
        }
    }

    if cache.is_file() {
        let bytes = tokio::fs::read(cache).await.io_at(cache)?;
        let manifest = parse_manifest(&bytes).map_err(|err| {
            let network = last_error
                .as_ref()
                .map(|err| err.to_string())
                .unwrap_or_else(|| "network request failed".into());
            Error::message(format!(
                "cached version manifest is invalid ({err}); {network}"
            ))
        })?;
        return Ok(ManifestFetch {
            manifest,
            from_cache: true,
        });
    }

    Err(last_error.unwrap_or_else(|| Error::message("could not download the version manifest")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> VersionManifest {
        parse_manifest(
            br#"{
              "latest": { "release": "1.21.1", "snapshot": "24w14a" },
              "versions": [
                {
                  "id": "24w14a",
                  "type": "snapshot",
                  "url": "https://example.invalid/24w14a.json",
                  "time": "2024-04-03T11:24:58+00:00",
                  "releaseTime": "2024-04-03T11:20:36+00:00",
                  "sha1": "abc",
                  "complianceLevel": 1
                },
                {
                  "id": "1.21.1",
                  "type": "release",
                  "url": "https://example.invalid/1.21.1.json",
                  "time": "2024-08-08T12:00:00+00:00",
                  "releaseTime": "2024-08-08T11:00:00+00:00"
                },
                {
                  "id": "b1.7.3",
                  "type": "old_beta",
                  "url": "https://example.invalid/b1.7.3.json",
                  "time": "2022-01-01T00:00:00+00:00",
                  "releaseTime": "2011-07-08T00:00:00+00:00",
                  "sha1": "def"
                }
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_latest_pointers_and_optional_sha1() {
        let manifest = sample();
        assert_eq!(manifest.latest.release, "1.21.1");
        assert_eq!(manifest.latest.snapshot, "24w14a");
        assert_eq!(manifest.versions.len(), 3);
        assert_eq!(
            manifest
                .find("24w14a")
                .and_then(|entry| entry.sha1.as_deref()),
            Some("abc")
        );
        assert!(manifest.find("1.21.1").unwrap().sha1.is_none());
        assert!(manifest.find("missing").is_none());
    }

    #[test]
    fn aliases_and_filters() {
        let manifest = sample();
        assert_eq!(manifest.resolve_id("release"), "1.21.1");
        assert_eq!(manifest.resolve_id("snapshot"), "24w14a");
        assert_eq!(manifest.resolve_id("1.21.1"), "1.21.1");

        let releases = manifest.query(Some(&[VersionKind::Release]), None);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].id, "1.21.1");

        let searched = manifest.query(None, Some("1.7"));
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].version_type, "old_beta");

        let modern = manifest.query(Some(&[VersionKind::Release, VersionKind::Snapshot]), None);
        assert_eq!(modern.len(), 2);
        assert_eq!(modern[0].id, "24w14a");
    }
}
