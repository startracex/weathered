//! Modrinth `.mrpack` archives.
//!
//! CurseForge packs are not imported. Their manifests name project and file
//! ids, and downloading those files needs a CurseForge API key that this
//! launcher does not ship.

use std::io::{Cursor, Read};

use serde::Deserialize;
use zip::ZipArchive;

use crate::error::{Error, Result};
use crate::paths::safe_rel_path;

/// A parsed Modrinth pack index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MrPack {
    /// Pack name.
    pub name: String,
    /// `dependencies.minecraft`.
    pub game_version: String,
    /// `vanilla`, `fabric`, `quilt`, `forge`, or `neoforge`.
    pub loader: String,
    /// Loader version. Empty for vanilla.
    pub loader_version: String,
    /// Client files that should be downloaded.
    pub files: Vec<MrFile>,
}

/// One file declared by a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MrFile {
    /// Path relative to the instance game directory, such as `mods/example.jar`.
    pub path: String,
    /// SHA-1, when the index published one.
    pub sha1: Option<String>,
    /// SHA-512, when the index published one.
    pub sha512: Option<String>,
    /// URLs to try, in order.
    pub downloads: Vec<String>,
    /// Size in bytes, when published.
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    name: String,
    files: Vec<IndexEntry>,
    dependencies: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct IndexEntry {
    path: String,
    hashes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    env: Option<IndexEnv>,
    downloads: Vec<String>,
    #[serde(rename = "fileSize")]
    file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct IndexEnv {
    client: Option<String>,
    #[allow(dead_code)]
    server: Option<String>,
}

/// Read `modrinth.index.json` from an `.mrpack` (a zip archive).
pub fn parse_mrpack(bytes: &[u8]) -> Result<MrPack> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| Error::message(format!("could not read modpack: {err}")))?;
    let mut index_bytes = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| Error::message(format!("could not read modpack entry: {err}")))?;
        let name = entry
            .name()
            .map_err(|err| Error::message(format!("modpack entry name: {err}")))?;
        if name == "modrinth.index.json" {
            let mut buffer = Vec::new();
            entry.read_to_end(&mut buffer).map_err(|err| {
                Error::message(format!("could not read modrinth.index.json: {err}"))
            })?;
            index_bytes = Some(buffer);
            break;
        }
    }
    let Some(index_bytes) = index_bytes else {
        return Err(Error::message(
            "modpack is missing modrinth.index.json. CurseForge packs are not supported.",
        ));
    };
    parse_index(&index_bytes)
}

/// Parse a Modrinth index document.
pub fn parse_index(bytes: &[u8]) -> Result<MrPack> {
    let index: IndexFile =
        serde_json::from_slice(bytes).map_err(|err| Error::json("modrinth.index.json", err))?;
    if index.format_version != 1 {
        return Err(Error::message(format!(
            "unsupported modpack format version {}",
            index.format_version
        )));
    }
    if index.name.trim().is_empty() {
        return Err(Error::message("modpack name is empty"));
    }
    let game_version = index
        .dependencies
        .get("minecraft")
        .cloned()
        .filter(|version| !version.is_empty())
        .ok_or_else(|| Error::message("modpack does not name a Minecraft version"))?;
    let (loader, loader_version) = loader_dependency(&index.dependencies)?;
    let mut files = Vec::new();
    for entry in index.files {
        if entry.env.as_ref().and_then(|env| env.client.as_deref()) == Some("unsupported") {
            continue;
        }
        safe_rel_path(&entry.path)?;
        if entry.downloads.is_empty() {
            return Err(Error::message(format!(
                "modpack file {} has no download URL",
                entry.path
            )));
        }
        files.push(MrFile {
            path: entry.path,
            sha1: entry.hashes.get("sha1").cloned(),
            sha512: entry.hashes.get("sha512").cloned(),
            downloads: entry.downloads,
            size: entry.file_size,
        });
    }
    Ok(MrPack {
        name: index.name,
        game_version,
        loader,
        loader_version,
        files,
    })
}

fn loader_dependency(
    dependencies: &std::collections::BTreeMap<String, String>,
) -> Result<(String, String)> {
    for (key, loader) in [
        ("fabric-loader", "fabric"),
        ("quilt-loader", "quilt"),
        ("neoforge", "neoforge"),
        ("forge", "forge"),
    ] {
        if let Some(version) = dependencies.get(key).filter(|version| !version.is_empty()) {
            return Ok((loader.to_string(), version.clone()));
        }
    }
    Ok(("vanilla".into(), String::new()))
}

/// Copy `overrides/` and `client-overrides/` into `game_dir`.
pub fn extract_overrides(bytes: &[u8], game_dir: &std::path::Path) -> Result<usize> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| Error::message(format!("could not read modpack: {err}")))?;
    let mut copied = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| Error::message(format!("could not read modpack entry: {err}")))?;
        let name = entry
            .name()
            .map_err(|err| Error::message(format!("modpack entry name: {err}")))?;
        let Some(relative) = override_path(&name) else {
            continue;
        };
        if entry.is_dir() {
            let dest = game_dir.join(&relative);
            std::fs::create_dir_all(&dest).map_err(|err| Error::io(&dest, err))?;
            continue;
        }
        let dest = game_dir.join(&relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Error::io(parent, err))?;
        }
        let mut output = std::fs::File::create(&dest).map_err(|err| Error::io(&dest, err))?;
        std::io::copy(&mut entry, &mut output).map_err(|err| Error::io(&dest, err))?;
        copied += 1;
    }
    Ok(copied)
}

fn override_path(name: &str) -> Option<std::path::PathBuf> {
    let rest = name
        .strip_prefix("overrides/")
        .or_else(|| name.strip_prefix("client-overrides/"))?;
    if rest.is_empty() {
        return None;
    }
    safe_rel_path(rest).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_pack() -> Vec<u8> {
        let index = r#"{
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": "example",
            "name": "Example Pack",
            "files": [
                {
                    "path": "mods/example.jar",
                    "hashes": { "sha1": "abc", "sha512": "def" },
                    "env": { "client": "required", "server": "required" },
                    "downloads": ["https://cdn.example.test/example.jar"],
                    "fileSize": 4
                },
                {
                    "path": "mods/server-only.jar",
                    "hashes": { "sha1": "fff" },
                    "env": { "client": "unsupported", "server": "required" },
                    "downloads": ["https://cdn.example.test/server.jar"],
                    "fileSize": 2
                }
            ],
            "dependencies": {
                "minecraft": "1.21.1",
                "fabric-loader": "0.16.9"
            }
        }"#;
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .start_file(
                    "modrinth.index.json",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(index.as_bytes()).unwrap();
            writer
                .start_file(
                    "overrides/config/example.txt",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(b"hello").unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn parses_fabric_pack_and_skips_server_only_files() {
        let pack = parse_mrpack(&sample_pack()).unwrap();
        assert_eq!(pack.name, "Example Pack");
        assert_eq!(pack.game_version, "1.21.1");
        assert_eq!(pack.loader, "fabric");
        assert_eq!(pack.loader_version, "0.16.9");
        assert_eq!(pack.files.len(), 1);
        assert_eq!(pack.files[0].path, "mods/example.jar");
        assert_eq!(pack.files[0].sha512.as_deref(), Some("def"));
    }

    #[test]
    fn overrides_are_copied_under_the_game_directory() {
        let root = std::env::temp_dir().join(format!(
            "weathered-mrpack-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let copied = extract_overrides(&sample_pack(), &root).unwrap();
        assert_eq!(copied, 1);
        assert_eq!(
            std::fs::read(root.join("config").join("example.txt")).unwrap(),
            b"hello"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
