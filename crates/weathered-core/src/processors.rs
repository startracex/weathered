//! Forge and NeoForge installer processors.
//!
//! Modern installers (Forge 1.13+ and NeoForge) ship `install_profile.json`
//! with a client-side processor list. Older installers that only contain an
//! `install` task are rejected with an explanation instead of a profile that
//! cannot launch.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use crate::cancel::CancelFlag;
use crate::error::{Error, Result};
use crate::library::{maven_rel_path, parse_maven};
use crate::paths::portable;

/// Parsed installer profile.
#[derive(Debug, Clone)]
pub struct InstallProfile {
    /// Minecraft version named by the profile, when present.
    pub minecraft: Option<String>,
    /// Client-side data values, with maven and embedded paths still raw.
    pub data: BTreeMap<String, String>,
    /// Processors that should run for a client install.
    pub processors: Vec<Processor>,
    /// Libraries the processors need on disk.
    pub libraries: Vec<crate::version::Library>,
    /// True when this is the modern processor format.
    pub supported: bool,
}

/// One processor invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processor {
    /// Maven coordinate of the jar that contains the main class.
    pub jar: String,
    /// Extra classpath coordinates.
    pub classpath: Vec<String>,
    /// Arguments before substitution.
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawProfile {
    #[serde(default)]
    minecraft: Option<String>,
    #[serde(default)]
    data: BTreeMap<String, RawSide>,
    #[serde(default)]
    processors: Vec<RawProcessor>,
    #[serde(default)]
    libraries: Vec<crate::version::Library>,
    #[serde(default)]
    install: Option<serde_json::Value>,
    #[serde(default)]
    spec: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RawSide {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawProcessor {
    jar: String,
    #[serde(default)]
    classpath: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    sides: Option<Vec<String>>,
}

/// Values substituted into processor arguments.
pub struct Subst<'a> {
    /// Directory that holds Maven-shaped jars.
    pub libraries: &'a Path,
    /// Vanilla client jar.
    pub minecraft_jar: &'a Path,
    /// Installer jar.
    pub installer: &'a Path,
    /// Directory embedded `/data` files were extracted into.
    pub extracted: &'a Path,
    /// Minecraft version id.
    pub minecraft_version: &'a str,
    /// Client-side data map.
    pub data: &'a BTreeMap<String, String>,
}

/// Read an installer profile and keep the client processors.
pub fn parse_install_profile(bytes: &[u8]) -> Result<InstallProfile> {
    let raw: RawProfile =
        serde_json::from_slice(bytes).map_err(|err| Error::json("install_profile.json", err))?;
    let modern = raw.spec.is_some() || !raw.processors.is_empty();
    if !modern && raw.install.is_some() {
        return Ok(InstallProfile {
            minecraft: raw.minecraft,
            data: BTreeMap::new(),
            processors: Vec::new(),
            libraries: raw.libraries,
            supported: false,
        });
    }
    let mut data = BTreeMap::new();
    for (key, value) in raw.data {
        if let Some(client) = value.client {
            data.insert(key, client);
        }
    }
    let processors = raw
        .processors
        .into_iter()
        .filter(|processor| match &processor.sides {
            None => true,
            Some(sides) => sides.iter().any(|side| side.eq_ignore_ascii_case("client")),
        })
        .map(|processor| Processor {
            jar: processor.jar,
            classpath: processor.classpath,
            args: processor.args,
        })
        .collect();
    Ok(InstallProfile {
        minecraft: raw.minecraft,
        data,
        processors,
        libraries: raw.libraries,
        supported: modern,
    })
}

/// Why a profile cannot be installed, when it is not the modern format.
pub fn unsupported_message(kind: &str) -> String {
    format!(
        "{kind} builds from before 1.13 use a different installer and cannot be launched. Choose a Forge 1.13+ or NeoForge build."
    )
}

/// Substitute one processor argument.
pub fn substitute(arg: &str, subst: &Subst<'_>) -> Result<String> {
    let replaced = replace_tokens(arg, subst);
    resolve_literal(&replaced, subst)
}

/// Run client processors. `java` is the binary selected for this game version.
pub fn run_processors(
    java: &Path,
    profile: &InstallProfile,
    subst: &Subst<'_>,
    cancel: &CancelFlag,
) -> Result<()> {
    if !profile.supported {
        return Err(Error::message(unsupported_message("This loader")));
    }
    for processor in &profile.processors {
        cancel.check()?;
        let jar = maven_file(&processor.jar, subst.libraries)?;
        let mut classpath = vec![jar.clone()];
        for entry in &processor.classpath {
            classpath.push(maven_file(entry, subst.libraries)?);
        }
        let main = jar_main_class(&jar)?;
        let mut args = Vec::new();
        for arg in &processor.args {
            args.push(substitute(arg, subst)?);
        }
        let separator = if cfg!(windows) { ";" } else { ":" };
        let cp = classpath
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(separator);
        let mut child = Command::new(java)
            .arg("-cp")
            .arg(cp)
            .arg(main)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                Error::message(format!("could not run processor {}: {err}", processor.jar))
            })?;
        loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                return Err(Error::message("cancelled"));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        let mut err = String::new();
                        if let Some(stderr) = child.stderr.as_mut() {
                            let _ = stderr.read_to_string(&mut err);
                        }
                        return Err(Error::message(format!(
                            "processor {} failed ({status}): {}",
                            processor.jar,
                            err.trim()
                        )));
                    }
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(err) => {
                    return Err(Error::message(format!(
                        "processor {} failed while waiting: {err}",
                        processor.jar
                    )));
                }
            }
        }
    }
    Ok(())
}

fn replace_tokens(input: &str, subst: &Subst<'_>) -> String {
    let mut out = input.to_string();
    let fixed = [
        ("{LIBRARY_DIR}", subst.libraries.display().to_string()),
        ("{MINECRAFT_JAR}", subst.minecraft_jar.display().to_string()),
        ("{INSTALLER}", subst.installer.display().to_string()),
        ("{ROOT}", subst.extracted.display().to_string()),
        ("{SIDE}", "client".into()),
        ("{MINECRAFT_VERSION}", subst.minecraft_version.to_string()),
    ];
    for (key, value) in fixed {
        out = out.replace(key, &value);
    }
    for (key, value) in subst.data {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

fn resolve_literal(value: &str, subst: &Subst<'_>) -> Result<String> {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|text| text.strip_suffix('\''))
    {
        return resolve_literal(inner, subst);
    }
    if let Some(inner) = value
        .strip_prefix('[')
        .and_then(|text| text.strip_suffix(']'))
    {
        return Ok(maven_file(inner, subst.libraries)?.display().to_string());
    }
    if let Some(path) = value.strip_prefix('/') {
        return Ok(subst.extracted.join(path).display().to_string());
    }
    Ok(value.to_string())
}

/// Path of a Maven coordinate inside `libraries`.
pub fn maven_file(spec: &str, libraries: &Path) -> Result<PathBuf> {
    let (coords, extension) = spec.split_once('@').unwrap_or((spec, "jar"));
    let mut parts = coords.split(':');
    let group = parts.next().unwrap_or("");
    let artifact = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    let classifier = parts.next().filter(|part| !part.is_empty());
    if group.is_empty() || artifact.is_empty() || version.is_empty() {
        return Err(Error::message(format!(
            "invalid processor artifact '{spec}'"
        )));
    }
    let mut name = format!("{group}:{artifact}:{version}");
    if let Some(classifier) = classifier {
        name.push(':');
        name.push_str(classifier);
    }
    if extension != "jar" {
        name.push('@');
        name.push_str(extension);
    }
    let coord = parse_maven(&name)?;
    Ok(libraries.join(maven_rel_path(&coord)))
}

fn jar_main_class(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|err| Error::io(path, err))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|err| Error::message(format!("could not read {}: {err}", path.display())))?;
    let mut entry = archive
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|_| Error::message(format!("{} has no manifest", path.display())))?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|err| Error::message(format!("could not read manifest: {err}")))?;
    let mut unfolded = String::new();
    for line in text.lines() {
        if line.starts_with(' ') {
            unfolded.push_str(line.trim_start());
        } else {
            unfolded.push('\n');
            unfolded.push_str(line);
        }
    }
    for line in unfolded.lines() {
        if let Some(class) = line.strip_prefix("Main-Class:") {
            let class = class.trim();
            if !class.is_empty() {
                return Ok(class.to_string());
            }
        }
    }
    Err(Error::message(format!(
        "{} does not declare a Main-Class",
        path.display()
    )))
}

/// Relative path helper used by tests.
pub fn display_maven(spec: &str) -> Result<String> {
    Ok(portable(&maven_file(spec, Path::new("libraries"))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_maven_paths_and_skips_server_processors() {
        let bytes = br#"{
          "spec": 1,
          "minecraft": "1.21.1",
          "data": {
            "BINPATCH": { "client": "/data/client.lzma", "server": "/data/server.lzma" },
            "MAPPINGS": { "client": "[net.minecraft:client:1.21.1:mappings@txt]" }
          },
          "processors": [
            {
              "sides": ["server"],
              "jar": "net.neoforged:tools:1",
              "args": ["server"]
            },
            {
              "sides": ["client"],
              "jar": "net.neoforged:tools:1",
              "classpath": ["org.ow2.asm:asm:9.8"],
              "args": ["--input", "{MINECRAFT_JAR}", "{BINPATCH}", "{MAPPINGS}", "{LIBRARY_DIR}"]
            }
          ]
        }"#;
        let profile = parse_install_profile(bytes).unwrap();
        assert!(profile.supported);
        assert_eq!(profile.processors.len(), 1);
        let libraries = Path::new("libraries");
        let subst = Subst {
            libraries,
            minecraft_jar: Path::new("client.jar"),
            installer: Path::new("installer.jar"),
            extracted: Path::new("work"),
            minecraft_version: "1.21.1",
            data: &profile.data,
        };
        assert_eq!(
            substitute("{BINPATCH}", &subst).unwrap().replace('\\', "/"),
            "work/data/client.lzma"
        );
        assert_eq!(
            substitute("{MAPPINGS}", &subst).unwrap().replace('\\', "/"),
            "libraries/net/minecraft/client/1.21.1/client-1.21.1-mappings.txt"
        );
        assert_eq!(
            display_maven("org.ow2.asm:asm:9.8").unwrap(),
            "libraries/org/ow2/asm/asm/9.8/asm-9.8.jar"
        );
    }

    #[test]
    fn legacy_installer_is_not_supported() {
        let profile = parse_install_profile(
            br#"{"install": {"profile": "forge"}, "versionInfo": {"id": "1.12.2-forge"}}"#,
        )
        .unwrap();
        assert!(!profile.supported);
        assert!(unsupported_message("Forge").contains("1.13"));
    }
}
