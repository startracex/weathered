//! Library classpath, native selection, and native extraction.

use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::download::RemoteFile;
use crate::error::{Error, IoResultExt, Result};
use crate::paths::{portable, safe_rel_path};
use crate::platform::{Arch, OsInfo};
use crate::rules::{FeatureSet, rules_allow};
use crate::version::Library;

const LIBRARIES_BASE: &str = "https://libraries.minecraft.net/";

/// A native jar that should be extracted into the natives directory.
#[derive(Debug, Clone)]
pub struct NativeJar {
    /// Path of the jar in `libraries/`.
    pub jar: PathBuf,
    /// Path prefixes to skip, such as `META-INF/`.
    pub exclude: Vec<String>,
}

/// Libraries selected for one operating system.
#[derive(Debug, Clone)]
pub struct LibraryPlan {
    /// Jars that belong on the classpath, in order, without the client jar.
    pub classpath: Vec<PathBuf>,
    /// Files that need to be present before launch.
    pub downloads: Vec<RemoteFile>,
    /// Native jars to extract for this OS and architecture.
    pub natives: Vec<NativeJar>,
}

/// Maven coordinate parsed from a library name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenCoord {
    /// Group id, dot-separated.
    pub group: String,
    /// Artifact id.
    pub artifact: String,
    /// Version.
    pub version: String,
    /// Optional classifier.
    pub classifier: Option<String>,
    /// File extension, usually `jar`.
    pub extension: String,
}

/// Parse `group:artifact:version[:classifier][:extension][@extension]`.
pub fn parse_maven(name: &str) -> Result<MavenCoord> {
    let (coords, extension_from_at) = match name.split_once('@') {
        Some((coords, extension)) => (coords, Some(extension.to_string())),
        None => (name, None),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    let (group, artifact, version, classifier, extension) = match parts.as_slice() {
        [group, artifact, version] => (*group, *artifact, *version, None, None),
        [group, artifact, version, classifier] => (
            *group,
            *artifact,
            *version,
            Some((*classifier).to_string()),
            None,
        ),
        [group, artifact, version, classifier, extension] => (
            *group,
            *artifact,
            *version,
            Some((*classifier).to_string()),
            Some((*extension).to_string()),
        ),
        _ => {
            return Err(Error::Maven {
                name: name.to_string(),
            });
        }
    };
    if group.is_empty() || artifact.is_empty() || version.is_empty() {
        return Err(Error::Maven {
            name: name.to_string(),
        });
    }
    Ok(MavenCoord {
        group: group.to_string(),
        artifact: artifact.to_string(),
        version: version.to_string(),
        classifier,
        extension: extension_from_at
            .or(extension)
            .unwrap_or_else(|| "jar".to_string()),
    })
}

/// Relative library path for a Maven coordinate.
pub fn maven_rel_path(coord: &MavenCoord) -> PathBuf {
    let mut path = PathBuf::new();
    for part in coord.group.split('.') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    path.push(&coord.artifact);
    path.push(&coord.version);
    let file = match &coord.classifier {
        Some(classifier) if !classifier.is_empty() => format!(
            "{}-{}-{}.{}",
            coord.artifact, coord.version, classifier, coord.extension
        ),
        _ => format!("{}-{}.{}", coord.artifact, coord.version, coord.extension),
    };
    path.push(file);
    path
}

/// Choose libraries, downloads, and natives for `os`.
pub fn plan_libraries(
    libraries: &[Library],
    libraries_dir: &Path,
    os: &OsInfo,
    features: &FeatureSet,
) -> Result<LibraryPlan> {
    let mut classpath = Vec::new();
    let mut downloads = Vec::new();
    let mut natives = Vec::new();
    let mut seen_names = HashSet::new();
    let mut seen_files = HashSet::new();

    for library in libraries {
        if !seen_names.insert(library.name.clone()) {
            continue;
        }
        let rules = library.rules.as_deref().unwrap_or(&[]);
        if !rules_allow(rules, os, features) {
            continue;
        }
        let coord = parse_maven(&library.name)?;
        if let Some(downloads_block) = &library.downloads {
            if let Some(artifact) = &downloads_block.artifact {
                let dest = libraries_dir.join(artifact_rel(artifact, &coord)?);
                stage_download(
                    &mut downloads,
                    &mut seen_files,
                    artifact,
                    dest.clone(),
                    false,
                    &library.name,
                )?;
                classpath.push(dest.clone());
                if let Some(classifier) = coord.classifier.as_deref()
                    && native_matches(classifier, os)
                {
                    natives.push(NativeJar {
                        jar: dest,
                        exclude: excludes(library),
                    });
                }
            }
        } else {
            let rel = maven_rel_path(&coord);
            let base = library.url.as_deref().unwrap_or(LIBRARIES_BASE);
            let dest = libraries_dir.join(&rel);
            let url = join_url(base, &portable(&rel));
            push_file(
                &mut downloads,
                &mut seen_files,
                RemoteFile {
                    url,
                    dest: dest.clone(),
                    sha1: library.sha1.clone(),
                    sha512: library.sha512.clone(),
                    size: library.size,
                    executable: false,
                },
            );
            classpath.push(dest);
        }

        if let Some(classifier) = mapped_native(library, os) {
            let classifiers = library
                .downloads
                .as_ref()
                .and_then(|downloads| downloads.classifiers.as_ref());
            let Some(artifact) = classifiers.and_then(|map| map.get(&classifier)) else {
                return Err(Error::message(format!(
                    "library {} is missing native classifier {classifier}",
                    library.name
                )));
            };
            let mut native_coord = coord.clone();
            native_coord.classifier = Some(classifier);
            let dest = libraries_dir.join(artifact_rel(artifact, &native_coord)?);
            stage_download(
                &mut downloads,
                &mut seen_files,
                artifact,
                dest.clone(),
                false,
                &library.name,
            )?;
            natives.push(NativeJar {
                jar: dest,
                exclude: excludes(library),
            });
        }
    }

    natives.sort_by_key(|native| portable(&native.jar).contains("patch"));
    Ok(LibraryPlan {
        classpath,
        downloads,
        natives,
    })
}

/// Whether a `natives-*` classifier should be extracted on this machine.
///
/// Unsuffixed classifiers such as `natives-linux` are the historical x86_64
/// builds. Architecture-specific classifiers are extracted only for that
/// architecture. `*-patch` classifiers are extracted for every architecture
/// of that OS.
pub fn native_matches(classifier: &str, os: &OsInfo) -> bool {
    let Some(rest) = classifier.strip_prefix("natives-") else {
        return false;
    };
    let (os_token, arch_token) = match rest.split_once('-') {
        Some((os_token, arch_token)) => (os_token, Some(arch_token)),
        None => (rest, None),
    };
    if !os_token_matches(os_token, os) {
        return false;
    }
    match arch_token {
        None => os.arch == Arch::X86_64,
        Some("patch") => true,
        Some("arm64" | "aarch64") => os.arch == Arch::Arm64,
        Some("arm32" | "arm") => os.arch == Arch::Arm32,
        Some("x86" | "32") => os.arch == Arch::X86,
        Some("x86_64" | "amd64" | "64") => os.arch == Arch::X86_64,
        Some(_) => false,
    }
}

fn os_token_matches(token: &str, os: &OsInfo) -> bool {
    match os.name {
        crate::platform::OsName::Windows => token.eq_ignore_ascii_case("windows"),
        crate::platform::OsName::Linux => token.eq_ignore_ascii_case("linux"),
        crate::platform::OsName::Osx => {
            token.eq_ignore_ascii_case("osx") || token.eq_ignore_ascii_case("macos")
        }
    }
}

fn mapped_native(library: &Library, os: &OsInfo) -> Option<String> {
    let natives = library.natives.as_ref()?;
    let template = natives.get(os.name.mojang())?;
    let bits = match os.arch {
        Arch::X86 | Arch::Arm32 => "32",
        _ => "64",
    };
    Some(template.replace("${arch}", bits))
}

fn excludes(library: &Library) -> Vec<String> {
    library
        .extract
        .as_ref()
        .and_then(|extract| extract.exclude.clone())
        .unwrap_or_default()
}

fn artifact_rel(
    artifact: &crate::version::ArtifactDownload,
    fallback: &MavenCoord,
) -> Result<PathBuf> {
    if let Some(path) = &artifact.path {
        safe_rel_path(path)
    } else {
        Ok(maven_rel_path(fallback))
    }
}

fn stage_download(
    downloads: &mut Vec<RemoteFile>,
    seen: &mut HashSet<PathBuf>,
    artifact: &crate::version::ArtifactDownload,
    dest: PathBuf,
    executable: bool,
    library_name: &str,
) -> Result<()> {
    let url = artifact.url().unwrap_or("").to_string();
    if url.is_empty() && !dest.is_file() {
        return Err(Error::message(format!(
            "library {library_name} has no download URL and {} is not present",
            dest.display()
        )));
    }
    push_file(
        downloads,
        seen,
        RemoteFile {
            url,
            dest,
            sha1: artifact.sha1.clone(),
            sha512: None,
            size: artifact.size,
            executable,
        },
    );
    Ok(())
}

fn push_file(downloads: &mut Vec<RemoteFile>, seen: &mut HashSet<PathBuf>, file: RemoteFile) {
    if file.url.is_empty() {
        return;
    }
    if seen.insert(file.dest.clone()) {
        downloads.push(file);
    }
}

fn join_url(base: &str, rel: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rel.trim_start_matches('/')
    )
}

/// Delete `path` if it exists and create it as an empty directory.
pub fn reset_dir(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path).io_at(path)?;
    }
    std::fs::create_dir_all(path).io_at(path)
}

/// Extract a native jar. Entries that escape the archive or match `exclude` are skipped.
pub fn extract_native_jar(jar: &Path, dest: &Path, exclude: &[String]) -> Result<usize> {
    let file = File::open(jar).io_at(jar)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| {
        Error::message(format!(
            "could not read native archive {}: {err}",
            jar.display()
        ))
    })?;
    std::fs::create_dir_all(dest).io_at(dest)?;
    let mut extracted = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| {
            Error::message(format!(
                "could not read entry {index} of {}: {err}",
                jar.display()
            ))
        })?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let relative = portable(&name);
        if relative.is_empty() || is_excluded(&relative, exclude) {
            continue;
        }
        let out = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).io_at(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        let mut output = File::create(&out).io_at(&out)?;
        std::io::copy(&mut entry, &mut output).map_err(|err| Error::io(&out, err))?;
        output.flush().io_at(&out)?;
        extracted += 1;
    }
    Ok(extracted)
}

fn is_excluded(relative: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|prefix| {
        let prefix = prefix.trim_end_matches(['/', '\\']);
        relative == prefix || relative.starts_with(&format!("{prefix}/"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Arch, OsName};
    use crate::rules::FeatureSet;
    use crate::version::parse_version;

    fn os(name: OsName, arch: Arch) -> OsInfo {
        OsInfo::new(name, arch, "10.0.19045")
    }

    #[test]
    fn maven_paths_include_the_classifier() {
        let plain = parse_maven("com.mojang:brigadier:1.0.17").unwrap();
        assert_eq!(
            portable(&maven_rel_path(&plain)),
            "com/mojang/brigadier/1.0.17/brigadier-1.0.17.jar"
        );
        let native = parse_maven("org.lwjgl:lwjgl:3.3.1:natives-linux").unwrap();
        assert_eq!(
            portable(&maven_rel_path(&native)),
            "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-linux.jar"
        );
        let extension = parse_maven("net.java.dev.jna:jna:5.13.0@jar").unwrap();
        assert_eq!(extension.extension, "jar");
        assert!(parse_maven("not-a-coordinate").is_err());
    }

    #[test]
    fn native_classifier_matches_only_the_right_arch() {
        let linux64 = os(OsName::Linux, Arch::X86_64);
        let linux_arm = os(OsName::Linux, Arch::Arm64);
        let mac_arm = os(OsName::Osx, Arch::Arm64);
        let windows32 = os(OsName::Windows, Arch::X86);
        assert!(native_matches("natives-linux", &linux64));
        assert!(!native_matches("natives-linux", &linux_arm));
        assert!(native_matches("natives-macos-arm64", &mac_arm));
        assert!(!native_matches("natives-macos", &mac_arm));
        assert!(native_matches("natives-macos-patch", &mac_arm));
        assert!(native_matches("natives-windows-x86", &windows32));
        assert!(native_matches(
            "natives-windows-64",
            &os(OsName::Windows, Arch::X86_64)
        ));
        assert!(!native_matches("linux-x86_64", &linux64));
    }

    #[test]
    fn old_style_natives_are_extracted_and_kept_off_the_classpath() {
        let version = parse_version(
            br#"{
              "id": "1.12.2",
              "libraries": [
                {
                  "name": "com.mojang:patchy:1.3.9",
                  "downloads": {
                    "artifact": {
                      "path": "com/mojang/patchy/1.3.9/patchy-1.3.9.jar",
                      "sha1": "abc",
                      "size": 10,
                      "url": "https://libraries.minecraft.net/com/mojang/patchy/1.3.9/patchy-1.3.9.jar"
                    }
                  }
                },
                {
                  "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
                  "extract": { "exclude": ["META-INF/"] },
                  "natives": { "linux": "natives-linux", "osx": "natives-osx", "windows": "natives-windows-${arch}" },
                  "downloads": {
                    "artifact": {
                      "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209.jar",
                      "sha1": "stub",
                      "size": 22,
                      "url": "https://libraries.minecraft.net/stub.jar"
                    },
                    "classifiers": {
                      "natives-linux": {
                        "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar",
                        "sha1": "linux",
                        "size": 100,
                        "url": "https://libraries.minecraft.net/natives-linux.jar"
                      },
                      "natives-windows-64": {
                        "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/natives-windows-64.jar",
                        "sha1": "win",
                        "size": 100,
                        "url": "https://libraries.minecraft.net/natives-windows-64.jar"
                      }
                    }
                  }
                },
                {
                  "name": "ca.weblite:java-objc-bridge:1.1",
                  "rules": [ { "action": "allow", "os": { "name": "osx" } } ],
                  "downloads": {
                    "artifact": {
                      "path": "ca/weblite/java-objc-bridge/1.1/java-objc-bridge-1.1.jar",
                      "sha1": "osx",
                      "size": 5,
                      "url": "https://libraries.minecraft.net/objc.jar"
                    }
                  }
                }
              ]
            }"#,
        )
        .unwrap();
        let libraries = version.libraries.unwrap();
        let plan = plan_libraries(
            &libraries,
            Path::new("libraries"),
            &os(OsName::Linux, Arch::X86_64),
            &FeatureSet::default(),
        )
        .unwrap();
        let classpath = plan
            .classpath
            .iter()
            .map(|path| portable(path))
            .collect::<Vec<_>>();
        assert_eq!(
            classpath,
            vec![
                "libraries/com/mojang/patchy/1.3.9/patchy-1.3.9.jar".to_string(),
                "libraries/org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209.jar".to_string()
            ]
        );
        assert_eq!(plan.natives.len(), 1);
        assert!(portable(&plan.natives[0].jar).ends_with("natives-linux.jar"));
        assert_eq!(plan.natives[0].exclude, vec!["META-INF/".to_string()]);
        assert!(
            plan.downloads
                .iter()
                .any(|file| file.sha1.as_deref() == Some("linux"))
        );
        assert!(!plan.downloads.iter().any(|file| file.url.contains("objc")));

        let windows = plan_libraries(
            &libraries,
            Path::new("libraries"),
            &os(OsName::Windows, Arch::X86_64),
            &FeatureSet::default(),
        )
        .unwrap();
        assert!(portable(&windows.natives[0].jar).contains("natives-windows-64"));
    }

    #[test]
    fn modern_natives_stay_on_the_classpath_and_only_the_matching_jar_is_extracted() {
        let version = parse_version(
            br#"{
              "id": "1.21.1",
              "libraries": [
                {
                  "name": "org.lwjgl:lwjgl:3.3.3",
                  "downloads": {
                    "artifact": {
                      "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar",
                      "sha1": "main",
                      "size": 10,
                      "url": "https://libraries.minecraft.net/lwjgl.jar"
                    }
                  }
                },
                {
                  "name": "org.lwjgl:lwjgl:3.3.3:natives-linux",
                  "rules": [ { "action": "allow", "os": { "name": "linux" } } ],
                  "downloads": {
                    "artifact": {
                      "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar",
                      "sha1": "nlinux",
                      "size": 10,
                      "url": "https://libraries.minecraft.net/natives-linux.jar"
                    }
                  }
                },
                {
                  "name": "org.lwjgl:lwjgl:3.3.3:natives-macos",
                  "rules": [ { "action": "allow", "os": { "name": "osx" } } ],
                  "downloads": {
                    "artifact": {
                      "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-macos.jar",
                      "sha1": "nmac",
                      "size": 10,
                      "url": "https://libraries.minecraft.net/natives-macos.jar"
                    }
                  }
                },
                {
                  "name": "org.lwjgl:lwjgl:3.3.3:natives-macos-arm64",
                  "rules": [ { "action": "allow", "os": { "name": "osx" } } ],
                  "downloads": {
                    "artifact": {
                      "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-macos-arm64.jar",
                      "sha1": "narm",
                      "size": 10,
                      "url": "https://libraries.minecraft.net/natives-macos-arm64.jar"
                    }
                  }
                },
                {
                  "name": "io.netty:netty-transport-native-epoll:4.1.97.Final:linux-x86_64",
                  "rules": [ { "action": "allow", "os": { "name": "linux" } } ],
                  "downloads": {
                    "artifact": {
                      "path": "io/netty/netty-transport-native-epoll/4.1.97.Final/netty-transport-native-epoll-4.1.97.Final-linux-x86_64.jar",
                      "sha1": "epoll",
                      "size": 10,
                      "url": "https://libraries.minecraft.net/epoll.jar"
                    }
                  }
                }
              ]
            }"#,
        )
        .unwrap();
        let libraries = version.libraries.unwrap();
        let linux = plan_libraries(
            &libraries,
            Path::new("libraries"),
            &os(OsName::Linux, Arch::X86_64),
            &FeatureSet::default(),
        )
        .unwrap();
        let names = linux
            .classpath
            .iter()
            .map(|path| portable(path))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(names.contains("lwjgl-3.3.3.jar"));
        assert!(names.contains("natives-linux.jar"));
        assert!(names.contains("linux-x86_64.jar"));
        assert!(!names.contains("natives-macos"));
        assert_eq!(linux.natives.len(), 1);
        assert!(portable(&linux.natives[0].jar).ends_with("natives-linux.jar"));

        let mac = plan_libraries(
            &libraries,
            Path::new("libraries"),
            &os(OsName::Osx, Arch::Arm64),
            &FeatureSet::default(),
        )
        .unwrap();
        assert_eq!(mac.natives.len(), 1);
        assert!(portable(&mac.natives[0].jar).contains("natives-macos-arm64"));
        assert!(
            mac.classpath
                .iter()
                .any(|path| portable(path).contains("natives-macos.jar"))
        );
        assert!(
            mac.classpath
                .iter()
                .any(|path| portable(path).contains("natives-macos-arm64.jar"))
        );
    }

    #[test]
    fn fabric_style_library_keeps_sha1_size_and_maven_url() {
        let version = parse_version(
            br#"{
              "id": "fabric-loader-0.20.1-26.3",
              "libraries": [
                {
                  "name": "net.fabricmc:fabric-loader:0.20.1",
                  "url": "https://maven.fabricmc.net/"
                },
                {
                  "name": "org.ow2.asm:asm:9.8",
                  "url": "https://maven.fabricmc.net/",
                  "sha1": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                  "sha512": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                  "size": 12345
                }
              ]
            }"#,
        )
        .unwrap();
        let plan = plan_libraries(
            version.libraries.as_deref().unwrap(),
            Path::new("libraries"),
            &os(OsName::Linux, Arch::X86_64),
            &FeatureSet::default(),
        )
        .unwrap();
        let loader = plan
            .downloads
            .iter()
            .find(|file| file.url.contains("fabric-loader"))
            .unwrap();
        assert_eq!(
            loader.url,
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.20.1/fabric-loader-0.20.1.jar"
        );
        assert!(loader.sha1.is_none());
        assert!(loader.size.is_none());
        let asm = plan
            .downloads
            .iter()
            .find(|file| file.url.contains("/asm/"))
            .unwrap();
        assert_eq!(
            asm.sha1.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(asm.sha512.as_deref().map(str::len), Some(128));
        assert_eq!(asm.size, Some(12345));
        assert!(asm.url.ends_with("org/ow2/asm/asm/9.8/asm-9.8.jar"));
    }
}
