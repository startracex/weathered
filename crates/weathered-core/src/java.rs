//! Finding a Java runtime, and downloading Mojang's if the machine does not have one.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::download::{self, sha1_hex, RemoteFile};
use crate::error::{Error, IoResultExt, Result};
use crate::paths::{safe_rel_path, GamePaths};
use crate::platform::{Arch, OsInfo, OsName};
use crate::progress::ProgressSink;
use crate::version::JavaVersionSpec;

const JAVA_MANIFEST_URL: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

/// A Java binary that can be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstallation {
    /// Path of `java` or `java.exe`.
    pub path: PathBuf,
    /// Major version. Java 8 is `8`, not `1`.
    pub major: u32,
    /// Version string from `java -version`, such as `21.0.2`.
    pub version: String,
    /// Where it was found: `PATH`, `JAVA_HOME`, `search`, `flag`, or `mojang:<component>`.
    pub source: String,
}

/// How install and launch should treat Mojang's runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaMode {
    /// Use a matching local runtime, and download Mojang's if none exists.
    Auto,
    /// Never download. Fail later if nothing suitable is installed.
    Skip,
    /// Always download Mojang's runtime for the version's component.
    ForceDownload,
}

/// Parsed `java -version` output.
pub fn parse_java_version(text: &str) -> Option<(u32, String)> {
    let marker = "version \"";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let version = &rest[..end];
    let major = parse_major(version)?;
    Some((major, version.to_string()))
}

fn parse_major(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '+', '-']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Run `java -version` and read the major version.
pub fn probe_java(path: &Path) -> Option<JavaInstallation> {
    let output = Command::new(path).arg("-version").output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    let (major, version) = parse_java_version(&text)?;
    Some(JavaInstallation {
        path: path.to_path_buf(),
        major,
        version,
        source: "probe".into(),
    })
}

/// JDKs and JREs visible through `JAVA_HOME`, `PATH`, and the usual install directories.
pub fn detect_system_javas() -> Vec<JavaInstallation> {
    let mut candidates = Vec::new();
    if let Some(path) = java_home_binary() {
        candidates.push((path, "JAVA_HOME".to_string()));
    }
    if let Some(path) = java_on_path() {
        candidates.push((path, "PATH".to_string()));
    }
    for root in search_roots() {
        collect_binaries(&root, 0, &mut candidates);
    }
    dedup_probes(candidates)
}

/// Runtimes previously downloaded into this game directory.
pub fn detect_vendored_runtimes(paths: &GamePaths) -> Vec<JavaInstallation> {
    let mut found = Vec::new();
    let Ok(components) = std::fs::read_dir(paths.runtime_root()) else {
        return found;
    };
    for component in components.flatten() {
        let Ok(platforms) = std::fs::read_dir(component.path()) else {
            continue;
        };
        for platform in platforms.flatten() {
            let Some(binary) = find_java_binary(&platform.path()) else {
                continue;
            };
            if let Some(mut installation) = probe_java(&binary) {
                let name = component.file_name();
                installation.source = format!("mojang:{}", name.to_string_lossy());
                found.push(installation);
            }
        }
    }
    found
}

/// Locate `java` under a Mojang runtime directory.
pub fn find_java_binary(root: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_binaries(root, 0, &mut found);
    found
        .into_iter()
        .map(|(path, _)| path)
        .min_by_key(|path| path.components().count())
}

/// Inputs for [`resolve_java`].
pub struct JavaRequest<'a> {
    /// Runtime requested by the version JSON.
    pub spec: Option<&'a JavaVersionSpec>,
    /// Binary chosen by the user.
    pub explicit: Option<&'a Path>,
    /// Whether Mojang's runtime may be downloaded.
    pub mode: JavaMode,
    /// OS that selects the runtime archive.
    pub os: &'a OsInfo,
    /// Concurrent downloads, used only when a runtime is fetched.
    pub concurrency: usize,
}

/// Choose a runtime for `request`, downloading Mojang's when the mode allows it.
pub async fn resolve_java(
    client: &reqwest::Client,
    paths: &GamePaths,
    request: JavaRequest<'_>,
    progress: &ProgressSink,
) -> Result<(JavaInstallation, download::DownloadStats)> {
    if let Some(path) = request.explicit {
        let mut installation = probe_java(path).ok_or_else(|| {
            Error::message(format!("could not execute Java at {}", path.display()))
        })?;
        installation.source = "flag".into();
        return Ok((installation, download::DownloadStats::default()));
    }

    if request.mode != JavaMode::ForceDownload {
        if let Some(spec) = request.spec {
            if let Some(vendored) = find_vendored(paths, &spec.component, request.os) {
                if vendored.major == spec.major_version {
                    return Ok((vendored, download::DownloadStats::default()));
                }
            }
            if let Some(system) = select_major(&detect_system_javas(), spec.major_version) {
                return Ok((system, download::DownloadStats::default()));
            }
        } else if let Some(system) = prefer_unspecified(&detect_system_javas()) {
            return Ok((system, download::DownloadStats::default()));
        }
    }

    if request.mode == JavaMode::Skip {
        let major = request.spec.map(|spec| spec.major_version).unwrap_or(0);
        return Err(Error::JavaNotFound { major });
    }

    let Some(spec) = request.spec else {
        return Err(Error::message(
            "this version does not say which Java runtime it needs, and no Java was found. Pass --java.",
        ));
    };
    install_mojang_runtime(
        client,
        paths,
        &spec.component,
        request.os,
        request.concurrency,
        progress,
    )
    .await
}

fn find_vendored(paths: &GamePaths, component: &str, os: &OsInfo) -> Option<JavaInstallation> {
    let key = runtime_platform_key(os).ok()?;
    let root = paths.runtime_dir(component, key);
    let binary = find_java_binary(&root)?;
    let mut installation = probe_java(&binary)?;
    installation.source = format!("mojang:{component}");
    Some(installation)
}

fn select_major(javas: &[JavaInstallation], major: u32) -> Option<JavaInstallation> {
    javas
        .iter()
        .filter(|java| java.major == major)
        .min_by_key(|java| source_rank(&java.source))
        .cloned()
}

fn prefer_unspecified(javas: &[JavaInstallation]) -> Option<JavaInstallation> {
    for major in [8, 17, 21, 16, 11] {
        if let Some(java) = select_major(javas, major) {
            return Some(java);
        }
    }
    javas.first().cloned()
}

fn source_rank(source: &str) -> u8 {
    match source {
        "JAVA_HOME" => 0,
        "PATH" => 1,
        _ if source.starts_with("mojang:") => 2,
        "search" => 3,
        _ => 4,
    }
}

/// Mojang runtime manifest key for this OS, such as `linux` or `windows-x64`.
pub fn runtime_platform_key(os: &OsInfo) -> Result<&'static str> {
    match (os.name, os.arch) {
        (OsName::Linux, Arch::X86) => Ok("linux-i386"),
        (OsName::Linux, Arch::X86_64) => Ok("linux"),
        (OsName::Osx, Arch::Arm64) => Ok("mac-os-arm64"),
        (OsName::Osx, _) => Ok("mac-os"),
        (OsName::Windows, Arch::X86) => Ok("windows-x86"),
        (OsName::Windows, Arch::Arm64) => Ok("windows-arm64"),
        (OsName::Windows, _) => Ok("windows-x64"),
        (OsName::Linux, Arch::Arm64) => Err(Error::message(
            "Mojang does not publish a Java runtime for Linux on ARM64. Install a matching JDK and pass --java.",
        )),
        (OsName::Linux, _) => Err(Error::message(
            "Mojang does not publish a Java runtime for this Linux architecture. Install a matching JDK and pass --java.",
        )),
    }
}

/// Download one Mojang Java component into the game directory.
pub async fn install_mojang_runtime(
    client: &reqwest::Client,
    paths: &GamePaths,
    component: &str,
    os: &OsInfo,
    concurrency: usize,
    progress: &ProgressSink,
) -> Result<(JavaInstallation, download::DownloadStats)> {
    let platform = runtime_platform_key(os)?;
    let catalog_bytes = download::get_bytes(client, JAVA_MANIFEST_URL).await?;
    let catalog: HashMap<String, HashMap<String, Vec<JavaRuntimeListing>>> =
        serde_json::from_slice(&catalog_bytes)
            .map_err(|err| Error::json("Java runtime catalog", err))?;
    let listing = catalog
        .get(platform)
        .and_then(|components| components.get(component))
        .and_then(|entries| entries.first())
        .ok_or_else(|| {
            Error::message(format!(
                "Mojang has no Java runtime '{component}' for {platform}"
            ))
        })?;
    let manifest_bytes = download::get_bytes(client, &listing.manifest.url).await?;
    if let Some(expected) = &listing.manifest.sha1 {
        let actual = sha1_hex(&manifest_bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::Checksum {
                path: paths.runtime_dir(component, platform).join("manifest.json"),
                expected: expected.clone(),
                actual,
            });
        }
    }
    let manifest: JavaRuntimeManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| Error::json(format!("Java runtime {component}"), err))?;
    let root = paths.runtime_dir(component, platform);
    let plan = plan_runtime(&manifest, &root)?;
    for directory in &plan.directories {
        tokio::fs::create_dir_all(directory)
            .await
            .io_at(directory)?;
    }
    let stats = download::download_all(client, plan.files, concurrency, "java", progress).await?;
    for link in &plan.links {
        create_runtime_link(&link.link, &link.target)?;
    }
    let binary = find_java_binary(&root).ok_or_else(|| {
        Error::message(format!(
            "downloaded Java runtime {component}, but no java binary was found in {}",
            root.display()
        ))
    })?;
    let mut installation = probe_java(&binary).ok_or_else(|| {
        Error::message(format!(
            "downloaded Java at {} does not run",
            binary.display()
        ))
    })?;
    installation.source = format!("mojang:{component} ({})", listing.version.name);
    Ok((installation, stats))
}

/// Files, directories, and symlinks described by a runtime manifest.
#[derive(Debug, Clone)]
pub struct RuntimePlan {
    /// Directories to create.
    pub directories: Vec<PathBuf>,
    /// Files to download.
    pub files: Vec<RemoteFile>,
    /// Symlinks to create after the files exist.
    pub links: Vec<RuntimeLink>,
}

/// A symlink inside a Java runtime.
#[derive(Debug, Clone)]
pub struct RuntimeLink {
    /// Link path.
    pub link: PathBuf,
    /// Target as published, usually relative.
    pub target: String,
}

/// Manifest of one Mojang Java runtime.
#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeManifest {
    /// Path to file, directory, or link.
    pub files: HashMap<String, JavaRuntimeFile>,
}

/// One entry in a runtime manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeFile {
    /// `file`, `directory`, or `link`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Whether the file should be executable.
    #[serde(default)]
    pub executable: bool,
    /// Symlink target.
    pub target: Option<String>,
    /// Download URLs. The raw file is used; lzma is ignored.
    pub downloads: Option<JavaRuntimeDownloads>,
}

/// Downloads for one runtime file.
#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeDownloads {
    /// Uncompressed file.
    pub raw: Option<RuntimeFileDownload>,
}

/// One runtime file download.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeFileDownload {
    /// SHA-1.
    pub sha1: Option<String>,
    /// Size in bytes.
    pub size: Option<u64>,
    /// Download URL.
    pub url: Option<String>,
}

/// Turn a runtime manifest into directories, downloads, and links.
pub fn plan_runtime(manifest: &JavaRuntimeManifest, root: &Path) -> Result<RuntimePlan> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut links = Vec::new();
    for (relative, file) in &manifest.files {
        let rel = safe_rel_path(relative)?;
        let dest = root.join(&rel);
        match file.kind.as_str() {
            "directory" => directories.push(dest),
            "file" => {
                let raw = file
                    .downloads
                    .as_ref()
                    .and_then(|downloads| downloads.raw.as_ref())
                    .ok_or_else(|| {
                        Error::message(format!("runtime file {relative} has no raw download"))
                    })?;
                files.push(RemoteFile {
                    url: raw.url.clone().unwrap_or_default(),
                    dest,
                    sha1: raw.sha1.clone(),
                    size: raw.size,
                    executable: file.executable,
                });
            }
            "link" => {
                let target = file.target.clone().ok_or_else(|| {
                    Error::message(format!("runtime link {relative} has no target"))
                })?;
                links.push(RuntimeLink { link: dest, target });
            }
            _ => {}
        }
    }
    Ok(RuntimePlan {
        directories,
        files,
        links,
    })
}

fn create_runtime_link(link: &Path, target: &str) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).io_at(parent)?;
    }
    if std::fs::symlink_metadata(link).is_ok() {
        let _ = std::fs::remove_file(link);
    }
    if symlink_to(target, link).is_ok() {
        return Ok(());
    }
    let resolved = link.parent().unwrap_or_else(|| Path::new(".")).join(target);
    if resolved.is_file() {
        std::fs::copy(&resolved, link).io_at(link)?;
        return Ok(());
    }
    Err(Error::message(format!(
        "could not link {} to {target}",
        link.display()
    )))
}

#[cfg(unix)]
fn symlink_to(target: &str, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_to(target: &str, link: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};
    let resolved = link.parent().unwrap_or_else(|| Path::new(".")).join(target);
    if resolved.is_dir() {
        symlink_dir(target, link)
    } else {
        symlink_file(target, link).or_else(|_| symlink_dir(target, link))
    }
}

fn java_home_binary() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("JAVA_HOME")?);
    let named = home.join("bin").join(java_exe());
    if named.is_file() {
        return Some(named);
    }
    if home.is_file() {
        return Some(home);
    }
    None
}

fn java_on_path() -> Option<PathBuf> {
    let finder = if cfg!(windows) { "where" } else { "which" };
    let output = Command::new(finder).arg(java_exe()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(PathBuf::from(first))
    }
}

fn java_exe() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(target_os = "linux")]
    {
        roots.push(PathBuf::from("/usr/lib/jvm"));
        roots.push(PathBuf::from("/usr/lib/java"));
        roots.push(PathBuf::from("/usr/local/lib/jvm"));
    }
    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Library/Java/JavaVirtualMachines"));
        }
    }
    #[cfg(windows)]
    {
        for base in [
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft",
            r"C:\Program Files\Zulu",
            r"C:\Program Files\BellSoft",
            r"C:\Program Files (x86)\Java",
        ] {
            roots.push(PathBuf::from(base));
        }
    }
    roots
}

fn collect_binaries(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, String)>) {
    if depth > 6 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_binaries(&path, depth + 1, out);
        } else if is_java_binary(&path) {
            out.push((path, "search".into()));
        }
    }
}

fn is_java_binary(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name != "java" && name != "java.exe" {
        return false;
    }
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("bin")
}

fn dedup_probes(candidates: Vec<(PathBuf, String)>) -> Vec<JavaInstallation> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for (path, source) in candidates {
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        if let Some(mut installation) = probe_java(&path) {
            installation.source = source;
            found.push(installation);
        }
    }
    found.sort_by(|left, right| {
        left.major
            .cmp(&right.major)
            .then_with(|| left.version.cmp(&right.version))
    });
    found
}

#[derive(Debug, Deserialize)]
struct JavaRuntimeListing {
    manifest: RuntimeManifestRef,
    version: JavaRuntimeVersionName,
}

#[derive(Debug, Deserialize)]
struct RuntimeManifestRef {
    sha1: Option<String>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct JavaRuntimeVersionName {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_and_java_8_version_strings() {
        let stderr = "openjdk version \"21.0.2\" 2024-01-16\nOpenJDK Runtime Environment\n";
        assert_eq!(parse_java_version(stderr), Some((21, "21.0.2".to_string())));
        let legacy = "java version \"1.8.0_392\"\nJava(TM) SE Runtime Environment\n";
        assert_eq!(
            parse_java_version(legacy),
            Some((8, "1.8.0_392".to_string()))
        );
        assert_eq!(
            parse_java_version("openjdk version \"17.0.9\"\n").map(|value| value.0),
            Some(17)
        );
        assert!(parse_java_version("not java").is_none());
    }

    #[test]
    fn runtime_plan_separates_files_directories_and_links() {
        let manifest: JavaRuntimeManifest = serde_json::from_str(
            r#"{
              "files": {
                "bin": { "type": "directory" },
                "bin/java": {
                  "type": "file",
                  "executable": true,
                  "downloads": {
                    "raw": { "sha1": "abc", "size": 12, "url": "https://example.invalid/java" },
                    "lzma": { "sha1": "def", "size": 4, "url": "https://example.invalid/java.lzma" }
                  }
                },
                "legal/java.base/LICENSE": {
                  "type": "link",
                  "target": "../java.base/LICENSE"
                }
              }
            }"#,
        )
        .unwrap();
        let plan = plan_runtime(&manifest, Path::new("runtime")).unwrap();
        assert_eq!(plan.directories.len(), 1);
        assert_eq!(plan.files.len(), 1);
        assert!(plan.files[0].executable);
        assert_eq!(plan.files[0].sha1.as_deref(), Some("abc"));
        assert_eq!(plan.links.len(), 1);
        assert_eq!(plan.links[0].target, "../java.base/LICENSE");
    }

    #[test]
    fn platform_keys_follow_mojang() {
        assert_eq!(
            runtime_platform_key(&OsInfo::new(OsName::Linux, Arch::X86_64, "")).unwrap(),
            "linux"
        );
        assert_eq!(
            runtime_platform_key(&OsInfo::new(OsName::Osx, Arch::Arm64, "")).unwrap(),
            "mac-os-arm64"
        );
        assert_eq!(
            runtime_platform_key(&OsInfo::new(OsName::Windows, Arch::X86_64, "")).unwrap(),
            "windows-x64"
        );
        assert!(runtime_platform_key(&OsInfo::new(OsName::Linux, Arch::Arm64, "")).is_err());
    }
}
