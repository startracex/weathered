//! Version JSON: parsing, inheritance, and conditional arguments.

use std::collections::HashSet;

use serde::Deserialize;

use crate::download::{self, sha1_hex};
use crate::error::{Error, IoResultExt, Result};
use crate::manifest::VersionManifest;
use crate::paths::{validate_version_id, GamePaths};
use crate::platform::OsInfo;
use crate::rules::{rules_allow, FeatureSet, Rule};

/// A resolved version, including every parent it inherits from.
#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    /// Merged version document. `inheritsFrom` is cleared.
    pub info: VersionInfo,
    /// Id of the version whose client jar should be launched.
    pub jar_id: String,
}

/// One Minecraft version document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// Version id.
    pub id: String,
    /// `release`, `snapshot`, `old_beta`, or `old_alpha`.
    #[serde(rename = "type")]
    pub version_type: Option<String>,
    /// Manifest update time.
    pub time: Option<String>,
    /// Original release time.
    pub release_time: Option<String>,
    /// Java main class.
    pub main_class: Option<String>,
    /// Minimum official launcher version.
    pub minimum_launcher_version: Option<u32>,
    /// Asset index name passed to the game.
    pub assets: Option<String>,
    /// Asset index download.
    pub asset_index: Option<AssetIndexRef>,
    /// Client and server downloads.
    pub downloads: Option<VersionDownloads>,
    /// Libraries. Absent when a child version wants to inherit the parent's list.
    pub libraries: Option<Vec<Library>>,
    /// Modern argument lists.
    pub arguments: Option<Arguments>,
    /// Legacy argument string used before 1.13.
    pub minecraft_arguments: Option<String>,
    /// Java runtime required by this version.
    pub java_version: Option<JavaVersionSpec>,
    /// Log4j configuration download.
    pub logging: Option<LoggingSpec>,
    /// Parent version id.
    pub inherits_from: Option<String>,
    /// Compliance level.
    pub compliance_level: Option<i32>,
}

/// Reference to an asset index.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    /// Index id, usually the same as [`VersionInfo::assets`].
    pub id: String,
    /// SHA-1 of the index document.
    pub sha1: String,
    /// Size of the index document.
    pub size: u64,
    /// Total size of the objects, when published.
    pub total_size: Option<u64>,
    /// Download URL.
    pub url: String,
}

/// Client and server artifacts.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionDownloads {
    /// The client jar.
    pub client: Option<ArtifactDownload>,
    /// The dedicated server jar.
    pub server: Option<ArtifactDownload>,
    /// Client obfuscation map.
    pub client_mappings: Option<ArtifactDownload>,
    /// Server obfuscation map.
    pub server_mappings: Option<ArtifactDownload>,
    /// Legacy Windows server jar.
    pub windows_server: Option<ArtifactDownload>,
}

/// A file described by URL, size, and SHA-1.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactDownload {
    /// SHA-1 of the file.
    pub sha1: Option<String>,
    /// Size in bytes.
    pub size: Option<u64>,
    /// Download URL. Empty or missing means the file is local.
    pub url: Option<String>,
    /// Path relative to `libraries/`.
    pub path: Option<String>,
}

impl ArtifactDownload {
    /// URL, if one was published.
    pub fn url(&self) -> Option<&str> {
        self.url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
    }
}

/// One library from the version JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    /// Maven coordinate (`group:artifact:version[:classifier]`).
    pub name: String,
    /// Artifact and native classifier downloads.
    pub downloads: Option<LibraryDownloads>,
    /// Legacy base URL. Joined with the Maven path when `downloads` is absent.
    pub url: Option<String>,
    /// OS and feature rules. Missing rules mean the library is included.
    pub rules: Option<Vec<Rule>>,
    /// Old-style native classifier names keyed by OS.
    pub natives: Option<std::collections::HashMap<String, String>>,
    /// Files to skip while extracting a native jar.
    pub extract: Option<ExtractSpec>,
}

/// Downloads attached to a library.
#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    /// The jar that goes on the classpath.
    pub artifact: Option<ArtifactDownload>,
    /// Native jars keyed by classifier, used by versions before 1.19.
    pub classifiers: Option<std::collections::HashMap<String, ArtifactDownload>>,
}

/// Extraction filter for a native jar.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtractSpec {
    /// Path prefixes to skip, such as `META-INF/`.
    pub exclude: Option<Vec<String>>,
}

/// Modern `arguments` object.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Arguments {
    /// Arguments passed to the game.
    pub game: Option<Vec<ArgumentValue>>,
    /// Arguments passed to the JVM before the main class.
    pub jvm: Option<Vec<ArgumentValue>>,
}

/// A plain argument, or one gated by rules.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ArgumentValue {
    /// Always included.
    Plain(String),
    /// Included when the rules allow it.
    Conditional {
        /// Rules evaluated against the current OS and features.
        rules: Vec<Rule>,
        /// One argument, or several.
        value: ArgumentPart,
    },
}

/// The `value` field of a conditional argument.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ArgumentPart {
    /// A single argument.
    One(String),
    /// Several arguments.
    Many(Vec<String>),
}

impl ArgumentPart {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

/// Java major version requested by a Minecraft version.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersionSpec {
    /// Mojang runtime component, such as `java-runtime-delta`.
    pub component: String,
    /// Required major version, such as 21.
    pub major_version: u32,
}

/// Logging configuration block.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingSpec {
    /// Client log config.
    pub client: Option<LoggingClient>,
}

/// Log4j config downloaded next to the assets.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingClient {
    /// JVM argument template. `${path}` is the downloaded file.
    pub argument: String,
    /// File to download.
    pub file: LoggingFile,
    /// Config type, usually `log4j2-xml`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// A logging config file.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingFile {
    /// File name.
    pub id: String,
    /// SHA-1.
    pub sha1: String,
    /// Size in bytes.
    pub size: u64,
    /// Download URL.
    pub url: String,
}

/// Parse a version JSON document.
pub fn parse_version(bytes: &[u8]) -> Result<VersionInfo> {
    serde_json::from_slice(bytes).map_err(|err| Error::json("version json", err))
}

/// Asset index name the game should be told about.
pub fn asset_index_name(version: &VersionInfo) -> Option<String> {
    version
        .assets
        .clone()
        .or_else(|| version.asset_index.as_ref().map(|index| index.id.clone()))
}

/// Merge `child` on top of `parent`.
///
/// Child libraries are prepended. Argument lists are concatenated, parent
/// first. A field the child omits keeps the parent's value.
pub fn merge_version(parent: VersionInfo, child: VersionInfo) -> VersionInfo {
    let mut merged = parent;
    merged.id = child.id;
    if child.version_type.is_some() {
        merged.version_type = child.version_type;
    }
    if child.time.is_some() {
        merged.time = child.time;
    }
    if child.release_time.is_some() {
        merged.release_time = child.release_time;
    }
    if child.main_class.is_some() {
        merged.main_class = child.main_class;
    }
    if child.minimum_launcher_version.is_some() {
        merged.minimum_launcher_version = child.minimum_launcher_version;
    }
    if child.assets.is_some() {
        merged.assets = child.assets;
    }
    if child.asset_index.is_some() {
        merged.asset_index = child.asset_index;
    }
    if child.downloads.is_some() {
        merged.downloads = child.downloads;
    }
    if child.java_version.is_some() {
        merged.java_version = child.java_version;
    }
    if child.logging.is_some() {
        merged.logging = child.logging;
    }
    if child.compliance_level.is_some() {
        merged.compliance_level = child.compliance_level;
    }
    if child.minecraft_arguments.is_some() {
        merged.minecraft_arguments = child.minecraft_arguments;
    }
    if let Some(child_libs) = child.libraries {
        let mut libs = child_libs;
        if let Some(parent_libs) = merged.libraries.take() {
            libs.extend(parent_libs);
        }
        merged.libraries = Some(dedup_libraries(libs));
    }
    merged.arguments = merge_arguments(merged.arguments, child.arguments);
    merged.inherits_from = None;
    merged
}

fn dedup_libraries(libraries: Vec<Library>) -> Vec<Library> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(libraries.len());
    for library in libraries {
        if seen.insert(library.name.clone()) {
            out.push(library);
        }
    }
    out
}

fn merge_arguments(parent: Option<Arguments>, child: Option<Arguments>) -> Option<Arguments> {
    match (parent, child) {
        (None, None) => None,
        (parent, None) => parent,
        (None, child) => child,
        (Some(parent), Some(child)) => Some(Arguments {
            game: concat_lists(parent.game, child.game),
            jvm: concat_lists(parent.jvm, child.jvm),
        }),
    }
}

fn concat_lists<T>(parent: Option<Vec<T>>, child: Option<Vec<T>>) -> Option<Vec<T>> {
    match (parent, child) {
        (None, None) => None,
        (Some(parent), None) => Some(parent),
        (None, Some(child)) => Some(child),
        (Some(mut parent), Some(child)) => {
            parent.extend(child);
            Some(parent)
        }
    }
}

/// Expand conditional arguments for one OS and feature set.
pub fn resolve_argument_list(
    values: &[ArgumentValue],
    os: &OsInfo,
    features: &FeatureSet,
) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        match value {
            ArgumentValue::Plain(text) => out.push(text.clone()),
            ArgumentValue::Conditional { rules, value } => {
                if rules_allow(rules, os, features) {
                    out.extend(value.clone().into_vec());
                }
            }
        }
    }
    out
}

/// Load a version and every parent it inherits from.
///
/// Documents that are already on disk and match the manifest SHA-1 are reused.
/// Missing documents are downloaded when `allow_network` is set.
pub async fn load_resolved(
    client: &reqwest::Client,
    paths: &GamePaths,
    manifest: &VersionManifest,
    id: &str,
    allow_network: bool,
) -> Result<ResolvedVersion> {
    let mut loaded = Vec::new();
    let mut seen = HashSet::new();
    let mut current = id.to_string();
    for _ in 0..8 {
        if !seen.insert(current.clone()) {
            return Err(Error::message(format!(
                "version inheritance cycle at {current}"
            )));
        }
        let info = load_one(client, paths, manifest, &current, allow_network).await?;
        let parent = info
            .inherits_from
            .clone()
            .filter(|parent| !parent.is_empty());
        loaded.push(info);
        match parent {
            Some(parent) => current = parent,
            None => {
                return Ok(fold_chain(loaded));
            }
        }
    }
    Err(Error::message(format!(
        "version {id} inherits from too many parents"
    )))
}

fn fold_chain(loaded: Vec<VersionInfo>) -> ResolvedVersion {
    let mut chain = loaded;
    chain.reverse();
    let root = chain.remove(0);
    let mut resolved = ResolvedVersion {
        jar_id: root.id.clone(),
        info: root,
    };
    for child in chain {
        if child
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.client.as_ref())
            .is_some()
        {
            resolved.jar_id = child.id.clone();
        }
        resolved.info = merge_version(resolved.info, child);
    }
    resolved
}

async fn load_one(
    client: &reqwest::Client,
    paths: &GamePaths,
    manifest: &VersionManifest,
    id: &str,
    allow_network: bool,
) -> Result<VersionInfo> {
    validate_version_id(id)?;
    let path = paths.version_json(id);
    let entry = manifest.find(id);
    if path.is_file() {
        let bytes = tokio::fs::read(&path).await.io_at(&path)?;
        let fresh = match entry.and_then(|entry| entry.sha1.as_deref()) {
            Some(expected) => sha1_hex(&bytes).eq_ignore_ascii_case(expected),
            None => true,
        };
        if fresh {
            return parse_version(&bytes).map_err(|err| annotate_version(id, err));
        }
    }

    let Some(entry) = entry else {
        if path.is_file() {
            let bytes = tokio::fs::read(&path).await.io_at(&path)?;
            return parse_version(&bytes).map_err(|err| annotate_version(id, err));
        }
        return Err(Error::VersionNotFound { id: id.to_string() });
    };
    if !allow_network {
        return Err(Error::message(format!(
            "version {id} is not installed. Run `weathered install {id}`"
        )));
    }

    let bytes = download::get_bytes(client, &entry.url).await?;
    if let Some(expected) = &entry.sha1 {
        let actual = sha1_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::Checksum {
                path,
                expected: expected.clone(),
                actual,
            });
        }
    }
    download::write_atomic(&path, &bytes).await?;
    parse_version(&bytes).map_err(|err| annotate_version(id, err))
}

fn annotate_version(id: &str, err: Error) -> Error {
    match err {
        Error::Json { source, .. } => Error::json(format!("version {id}"), source),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Arch, OsName};
    use crate::rules::FeatureSet;

    #[test]
    fn merges_inheritance_libraries_and_arguments() {
        let parent = parse_version(
            br#"{
              "id": "1.20.1",
              "type": "release",
              "mainClass": "net.minecraft.client.main.Main",
              "assets": "5",
              "libraries": [ { "name": "com.mojang:parent:1" } ],
              "arguments": {
                "game": [ "--username", "${auth_player_name}" ],
                "jvm": [ "-Xms1G" ]
              }
            }"#,
        )
        .unwrap();
        let child = parse_version(
            br#"{
              "id": "fabric-loader",
              "inheritsFrom": "1.20.1",
              "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
              "libraries": [
                { "name": "net.fabricmc:loader:0.15.0" },
                { "name": "com.mojang:parent:1" }
              ],
              "arguments": { "jvm": [ "-Dfabric" ] }
            }"#,
        )
        .unwrap();

        let merged = merge_version(parent, child);
        assert_eq!(merged.id, "fabric-loader");
        assert!(merged.inherits_from.is_none());
        assert_eq!(
            merged.main_class.as_deref(),
            Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
        );
        assert_eq!(merged.assets.as_deref(), Some("5"));
        let names: Vec<_> = merged
            .libraries
            .unwrap()
            .into_iter()
            .map(|library| library.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "net.fabricmc:loader:0.15.0".to_string(),
                "com.mojang:parent:1".to_string()
            ]
        );
        let arguments = merged.arguments.unwrap();
        assert_eq!(
            arguments.jvm.unwrap(),
            vec![
                ArgumentValue::Plain("-Xms1G".into()),
                ArgumentValue::Plain("-Dfabric".into())
            ]
        );
        assert_eq!(
            arguments.game.unwrap(),
            vec![
                ArgumentValue::Plain("--username".into()),
                ArgumentValue::Plain("${auth_player_name}".into())
            ]
        );
    }

    #[test]
    fn parses_legacy_and_conditional_arguments() {
        let modern = parse_version(
            br#"{
              "id": "1.21.1",
              "mainClass": "net.minecraft.client.main.Main",
              "arguments": {
                "jvm": [
                  { "rules": [ { "action": "allow", "os": { "name": "osx" } } ], "value": [ "-XstartOnFirstThread" ] },
                  { "rules": [ { "action": "allow", "os": { "name": "windows" } } ], "value": "-XX:HeapDumpPath=heapdump" },
                  "-cp",
                  "${classpath}"
                ],
                "game": [
                  "--username",
                  "${auth_player_name}",
                  { "rules": [ { "action": "allow", "features": { "is_demo_user": true } } ], "value": "--demo" }
                ]
              }
            }"#,
        )
        .unwrap();
        let jvm = modern.arguments.unwrap().jvm.unwrap();
        let linux = OsInfo::new(OsName::Linux, Arch::X86_64, "");
        let resolved = resolve_argument_list(&jvm, &linux, &FeatureSet::default());
        assert_eq!(
            resolved,
            vec!["-cp".to_string(), "${classpath}".to_string()]
        );

        let legacy = parse_version(
            br#"{
              "id": "1.12.2",
              "minecraftArguments": "--username ${auth_player_name} --version ${version_name}"
            }"#,
        )
        .unwrap();
        assert!(legacy.arguments.is_none());
        assert!(legacy
            .minecraft_arguments
            .unwrap()
            .contains("${auth_player_name}"));
    }

    #[test]
    fn child_without_a_client_keeps_the_parent_jar_id() {
        let parent = parse_version(
            br#"{
              "id": "1.20.1",
              "downloads": { "client": { "sha1": "abc", "size": 10, "url": "https://example.invalid/client.jar" } }
            }"#,
        )
        .unwrap();
        let child = parse_version(br#"{ "id": "fabric", "inheritsFrom": "1.20.1" }"#).unwrap();
        let resolved = fold_chain(vec![child, parent]);
        assert_eq!(resolved.info.id, "fabric");
        assert_eq!(resolved.jar_id, "1.20.1");
        assert!(resolved.info.downloads.unwrap().client.is_some());
    }
}
