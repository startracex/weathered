//! Build the JVM command from a version JSON and start the game.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::assets::AssetIndex;
use crate::auth::Session;
use crate::error::{Error, IoResultExt, Result};
use crate::java::{self, JavaInstallation, JavaMode};
use crate::library::{self, plan_libraries};
use crate::manifest;
use crate::paths::GamePaths;
use crate::platform::OsInfo;
use crate::progress::ProgressSink;
use crate::rules::FeatureSet;
use crate::version::{self, resolve_argument_list, VersionInfo};
use crate::{LAUNCHER_NAME, LAUNCHER_VERSION};

/// Everything required to build a command line.
///
/// Paths are stored owned so a GUI can construct a plan without borrowing the
/// installer. `prepare_launch` fills this from disk.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Merged version JSON.
    pub version: VersionInfo,
    /// Library jars, not including the client jar.
    pub classpath: Vec<PathBuf>,
    /// Client jar.
    pub client_jar: PathBuf,
    /// Directory passed as `${natives_directory}`.
    pub natives_dir: PathBuf,
    /// Game directory passed as `${game_directory}`.
    pub game_dir: PathBuf,
    /// `assets/` directory.
    pub assets_dir: PathBuf,
    /// Value of `${assets_index_name}`.
    pub assets_index_name: String,
    /// Value of `${game_assets}` for legacy versions.
    pub game_assets: PathBuf,
    /// `libraries/` directory.
    pub libraries_dir: PathBuf,
    /// Already substituted logging argument, when the config file exists.
    pub log_argument: Option<String>,
    /// Player session.
    pub session: Session,
    /// Java binary to execute.
    pub java: JavaInstallation,
    /// Heap size passed to `-Xmx`, such as `2G`. Empty skips the flag.
    pub memory: String,
    /// OS used to evaluate rules. This is not always the host OS in tests.
    pub os: OsInfo,
    /// Feature flags for conditional arguments.
    pub features: FeatureSet,
    /// Value of `${clientid}`.
    pub client_id: String,
    /// Quick Play values. Empty when that mode is off.
    pub quick_play_path: String,
    /// Singleplayer world name.
    pub quick_play_singleplayer: String,
    /// Multiplayer address.
    pub quick_play_multiplayer: String,
    /// Realm id.
    pub quick_play_realms: String,
    /// Width, when custom resolution is enabled.
    pub width: Option<u32>,
    /// Height, when custom resolution is enabled.
    pub height: Option<u32>,
    /// `${launcher_name}`.
    pub launcher_name: String,
    /// `${launcher_version}`.
    pub launcher_version: String,
    /// Where a Java argument file is written if the command line is very long.
    pub argfile: PathBuf,
    /// Shown when the chosen Java major does not match the version JSON.
    pub java_warning: Option<String>,
}

/// A command that is ready to spawn.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// Java binary.
    pub java: PathBuf,
    /// Java major version.
    pub java_major: u32,
    /// Java version string.
    pub java_version: String,
    /// Where Java was found.
    pub java_source: String,
    /// JVM arguments, with placeholders already replaced.
    pub jvm_args: Vec<String>,
    /// Main class.
    pub main_class: String,
    /// Game arguments, with placeholders already replaced.
    pub game_args: Vec<String>,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Argument file used when the command would be too long.
    pub argfile: PathBuf,
    /// Version id.
    pub version_id: String,
    /// Player name.
    pub username: String,
    /// Access token. Display hides this when it is a real token.
    pub access_token: String,
    /// Optional warning about the selected Java runtime.
    pub java_warning: Option<String>,
}

/// Options for [`prepare_launch`].
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    /// Version id, or `release` / `snapshot`.
    pub version_id: String,
    /// Player session.
    pub session: Session,
    /// Java binary. Discovered when absent.
    pub java: Option<PathBuf>,
    /// `-Xmx` value.
    pub memory: String,
    /// Target OS.
    pub os: OsInfo,
    /// Feature flags.
    pub features: FeatureSet,
    /// Game client id.
    pub client_id: String,
    /// Quick Play path.
    pub quick_play_path: String,
    /// Quick Play singleplayer world.
    pub quick_play_singleplayer: String,
    /// Quick Play server.
    pub quick_play_multiplayer: String,
    /// Quick Play realm.
    pub quick_play_realms: String,
    /// Window width.
    pub width: Option<u32>,
    /// Window height.
    pub height: Option<u32>,
    /// Whether a missing runtime may be downloaded.
    pub java_mode: JavaMode,
    /// Download concurrency, used only if a runtime is fetched.
    pub concurrency: usize,
}

/// Load an installed version, check its files, and build the launch command.
pub async fn prepare_launch(
    client: &reqwest::Client,
    paths: &GamePaths,
    request: LaunchRequest,
    progress: &ProgressSink,
) -> Result<LaunchPlan> {
    let fetched = manifest::fetch_manifest(client, &paths.manifest_cache()).await?;
    let version_id = fetched.manifest.resolve_id(&request.version_id).to_string();
    let resolved =
        version::load_resolved(client, paths, &fetched.manifest, &version_id, true).await?;
    let libraries = resolved.info.libraries.clone().unwrap_or_default();
    let library_plan = plan_libraries(
        &libraries,
        &paths.libraries(),
        &request.os,
        &request.features,
    )?;
    let client_jar = paths.version_jar(&resolved.jar_id);
    ensure_installed(&version_id, &client_jar, &library_plan)?;

    let natives_dir = paths.natives_dir(&version_id);
    let natives = library_plan.natives.clone();
    let natives_for_task = natives_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        library::reset_dir(&natives_for_task)?;
        for native in &natives {
            library::extract_native_jar(&native.jar, &natives_for_task, &native.exclude)?;
        }
        for sub in ["java", "jna", "lwjgl", "netty"] {
            let path = natives_for_task.join(sub);
            std::fs::create_dir_all(&path).io_at(&path)?;
        }
        Ok(())
    })
    .await
    .map_err(|err| Error::message(format!("could not extract natives: {err}")))??;

    let (java, _) = java::resolve_java(
        client,
        paths,
        java::JavaRequest {
            spec: resolved.info.java_version.as_ref(),
            explicit: request.java.as_deref(),
            mode: request.java_mode,
            os: &request.os,
            concurrency: request.concurrency,
        },
        progress,
    )
    .await?;
    let java_warning = java_mismatch(&resolved.info, &java, request.java.is_some());

    let index_name = version::asset_index_name(&resolved.info).unwrap_or_default();
    let game_assets = game_assets_dir(paths, &index_name);
    let log_argument = logging_argument(&resolved.info, paths);

    build_launch_plan(LaunchSpec {
        version: resolved.info,
        classpath: library_plan.classpath,
        client_jar,
        natives_dir,
        game_dir: paths.root.clone(),
        assets_dir: paths.assets(),
        assets_index_name: index_name,
        game_assets,
        libraries_dir: paths.libraries(),
        log_argument,
        session: request.session,
        java,
        memory: request.memory,
        os: request.os,
        features: request.features,
        client_id: request.client_id,
        quick_play_path: request.quick_play_path,
        quick_play_singleplayer: request.quick_play_singleplayer,
        quick_play_multiplayer: request.quick_play_multiplayer,
        quick_play_realms: request.quick_play_realms,
        width: request.width,
        height: request.height,
        launcher_name: LAUNCHER_NAME.into(),
        launcher_version: LAUNCHER_VERSION.into(),
        argfile: paths.argfile(),
        java_warning,
    })
}

fn java_mismatch(version: &VersionInfo, java: &JavaInstallation, explicit: bool) -> Option<String> {
    let spec = version.java_version.as_ref()?;
    if java.major == spec.major_version {
        return None;
    }
    let message = format!(
        "Java {} does not match the major version {} required by {} ({})",
        java.major, spec.major_version, version.id, spec.component
    );
    if explicit {
        Some(format!("{message}. Continuing because --java was set."))
    } else {
        Some(message)
    }
}

fn ensure_installed(
    version_id: &str,
    client_jar: &Path,
    plan: &library::LibraryPlan,
) -> Result<()> {
    let mut missing = Vec::new();
    if !client_jar.is_file() {
        missing.push(client_jar.to_path_buf());
    }
    for path in plan
        .classpath
        .iter()
        .chain(plan.natives.iter().map(|native| &native.jar))
    {
        if !path.is_file() {
            missing.push(path.clone());
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let shown = missing
        .iter()
        .take(5)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error::message(format!(
        "version {version_id} is missing {} file(s), including {shown}. Run `weathered install {version_id}`",
        missing.len()
    )))
}

pub(crate) fn game_assets_dir(paths: &GamePaths, index_name: &str) -> PathBuf {
    if index_name.is_empty() {
        return paths.assets();
    }
    let index_path = paths.asset_index(index_name);
    if let Ok(bytes) = std::fs::read(&index_path) {
        if let Ok(index) = serde_json::from_slice::<AssetIndex>(&bytes) {
            if index.map_to_resources {
                return paths.resources();
            }
            if index.virtual_assets {
                return paths.virtual_assets(index_name);
            }
        }
    }
    paths.assets()
}

pub(crate) fn logging_argument(version: &VersionInfo, paths: &GamePaths) -> Option<String> {
    let logging = version.logging.as_ref()?.client.as_ref()?;
    let path = paths.log_config(&logging.file.id);
    if !path.is_file() {
        return None;
    }
    Some(
        logging
            .argument
            .replace("${path}", &path.display().to_string()),
    )
}

/// Substitute placeholders and apply rules.
pub fn build_launch_plan(spec: LaunchSpec) -> Result<LaunchPlan> {
    let main_class = spec.version.main_class.clone().ok_or_else(|| {
        Error::message(format!(
            "version {} does not declare a main class",
            spec.version.id
        ))
    })?;
    let classpath = join_classpath(
        &spec.classpath,
        &spec.client_jar,
        spec.os.classpath_separator(),
    );
    let variables = variables(&spec, &classpath);

    let mut jvm = Vec::new();
    if let Some(memory) = memory_arg(&spec.memory) {
        jvm.push(memory);
    }
    if let Some(args) = spec
        .version
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.jvm.as_ref())
    {
        jvm.extend(resolve_argument_list(args, &spec.os, &spec.features));
    } else {
        jvm.push("-Djava.library.path=${natives_directory}".into());
        jvm.push("-Dminecraft.launcher.brand=${launcher_name}".to_string());
        jvm.push("-Dminecraft.launcher.version=${launcher_version}".to_string());
        jvm.push("-cp".into());
        jvm.push("${classpath}".into());
    }
    let jvm = jvm
        .iter()
        .map(|arg| substitute(arg, &variables))
        .collect::<Vec<_>>();
    let mut jvm = jvm;
    if !jvm.iter().any(|arg| arg == "-cp" || arg == "-classpath") {
        jvm.push("-cp".into());
        jvm.push(classpath);
    }
    if let Some(log_argument) = spec.log_argument.clone() {
        jvm.push(substitute(&log_argument, &variables));
    }

    let game_template = if let Some(args) = spec
        .version
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.game.as_ref())
    {
        resolve_argument_list(args, &spec.os, &spec.features)
    } else if let Some(template) = &spec.version.minecraft_arguments {
        template.split_whitespace().map(str::to_string).collect()
    } else {
        return Err(Error::message(format!(
            "version {} has no launch arguments",
            spec.version.id
        )));
    };
    let game_args = game_template
        .iter()
        .map(|arg| substitute(arg, &variables))
        .collect();

    Ok(LaunchPlan {
        java: spec.java.path,
        java_major: spec.java.major,
        java_version: spec.java.version,
        java_source: spec.java.source,
        jvm_args: jvm,
        main_class,
        game_args,
        working_dir: spec.game_dir,
        argfile: spec.argfile,
        version_id: spec.version.id,
        username: spec.session.username,
        access_token: spec.session.access_token,
        java_warning: spec.java_warning,
    })
}

fn memory_arg(memory: &str) -> Option<String> {
    let memory = memory.trim();
    if memory.is_empty() || memory == "0" {
        None
    } else {
        Some(format!("-Xmx{memory}"))
    }
}

/// Insert a minimum heap and extra arguments ahead of the version's JVM args.
pub fn apply_jvm_overrides(plan: &mut LaunchPlan, memory_min: &str, extra: &str) {
    if let Some(min) = memory_flag("-Xms", memory_min) {
        plan.jvm_args.insert(0, min);
    }
    let extra: Vec<String> = extra.split_whitespace().map(str::to_string).collect();
    if extra.is_empty() {
        return;
    }
    let at = plan
        .jvm_args
        .iter()
        .position(|arg| arg.starts_with("-Xmx"))
        .map(|index| index + 1)
        .unwrap_or(0);
    for (offset, arg) in extra.into_iter().enumerate() {
        plan.jvm_args.insert(at + offset, arg);
    }
}

fn memory_flag(flag: &str, memory: &str) -> Option<String> {
    let memory = memory.trim();
    if memory.is_empty() || memory == "0" {
        None
    } else {
        Some(format!("{flag}{memory}"))
    }
}

fn join_classpath(libraries: &[PathBuf], client: &Path, separator: &str) -> String {
    let mut parts: Vec<String> = libraries
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    parts.push(client.display().to_string());
    parts.join(separator)
}

fn variables(spec: &LaunchSpec, classpath: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    vars.insert("auth_player_name".into(), spec.session.username.clone());
    vars.insert("version_name".into(), spec.version.id.clone());
    vars.insert("game_directory".into(), spec.game_dir.display().to_string());
    vars.insert("assets_root".into(), spec.assets_dir.display().to_string());
    vars.insert("game_assets".into(), spec.game_assets.display().to_string());
    vars.insert("assets_index_name".into(), spec.assets_index_name.clone());
    vars.insert("auth_uuid".into(), spec.session.uuid.clone());
    vars.insert(
        "auth_access_token".into(),
        spec.session.access_token.clone(),
    );
    vars.insert(
        "auth_session".into(),
        format!("token:{}:{}", spec.session.access_token, spec.session.uuid),
    );
    vars.insert("user_type".into(), spec.session.user_type.clone());
    vars.insert(
        "version_type".into(),
        spec.version
            .version_type
            .clone()
            .unwrap_or_else(|| "release".into()),
    );
    vars.insert("user_properties".into(), "{}".into());
    vars.insert("clientid".into(), spec.client_id.clone());
    vars.insert("auth_xuid".into(), spec.session.xuid.clone());
    vars.insert(
        "natives_directory".into(),
        spec.natives_dir.display().to_string(),
    );
    vars.insert("launcher_name".into(), spec.launcher_name.clone());
    vars.insert("launcher_version".into(), spec.launcher_version.clone());
    vars.insert("classpath".into(), classpath.to_string());
    vars.insert(
        "classpath_separator".into(),
        spec.os.classpath_separator().to_string(),
    );
    vars.insert(
        "library_directory".into(),
        spec.libraries_dir.display().to_string(),
    );
    vars.insert("primary_jar".into(), spec.client_jar.display().to_string());
    vars.insert(
        "resolution_width".into(),
        spec.width
            .map(|width| width.to_string())
            .unwrap_or_default(),
    );
    vars.insert(
        "resolution_height".into(),
        spec.height
            .map(|height| height.to_string())
            .unwrap_or_default(),
    );
    vars.insert("quickPlayPath".into(), spec.quick_play_path.clone());
    vars.insert(
        "quickPlaySingleplayer".into(),
        spec.quick_play_singleplayer.clone(),
    );
    vars.insert(
        "quickPlayMultiplayer".into(),
        spec.quick_play_multiplayer.clone(),
    );
    vars.insert("quickPlayRealms".into(), spec.quick_play_realms.clone());
    vars
}

/// Replace every `${name}` in `input`. Unknown names are left as written.
pub fn substitute(input: &str, variables: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            let key = &after[..end];
            if let Some(value) = variables.get(key) {
                out.push_str(value);
            } else {
                out.push_str("${");
                out.push_str(key);
                out.push('}');
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

impl LaunchPlan {
    /// Rough length of the command line, used to decide when to use an argument file.
    pub fn command_len(&self) -> usize {
        self.java.as_os_str().len()
            + self.main_class.len()
            + self.jvm_args.iter().map(String::len).sum::<usize>()
            + self.game_args.iter().map(String::len).sum::<usize>()
            + self.jvm_args.len()
            + self.game_args.len()
    }

    /// Java 9 and newer can read `@argfile`. Java 8 cannot, so it always uses argv.
    pub fn uses_argfile(&self) -> bool {
        self.java_major >= 9 && self.command_len() > 7_000
    }

    /// Command line for display. A real Microsoft token is replaced with `***`.
    pub fn display_command(&self) -> String {
        let mut parts = Vec::new();
        parts.push(quote_shell(&self.java.display().to_string()));
        if self.uses_argfile() {
            parts.push(format!(
                "@{}",
                quote_shell(&self.argfile.display().to_string())
            ));
        } else {
            for arg in self
                .jvm_args
                .iter()
                .chain(std::iter::once(&self.main_class))
                .chain(self.game_args.iter())
            {
                parts.push(quote_shell(&self.mask(arg)));
            }
        }
        parts.join(" ")
    }

    fn mask<'a>(&'a self, arg: &'a str) -> std::borrow::Cow<'a, str> {
        if !self.access_token.is_empty() && self.access_token != "0" && arg == self.access_token {
            std::borrow::Cow::Borrowed("***")
        } else {
            std::borrow::Cow::Borrowed(arg)
        }
    }
}

/// Start the game with the terminal attached and return the child process.
pub fn spawn(plan: &LaunchPlan) -> Result<Child> {
    let mut command = prepare_command(plan)?;
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
        .spawn()
        .map_err(|err| Error::message(format!("failed to start {}: {err}", plan.java.display())))
}

/// Start the game with stdout and stderr piped, so a TUI can show the log.
pub fn spawn_captured(plan: &LaunchPlan) -> Result<Child> {
    let mut command = prepare_command(plan)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .spawn()
        .map_err(|err| Error::message(format!("failed to start {}: {err}", plan.java.display())))
}

fn prepare_command(plan: &LaunchPlan) -> Result<Command> {
    let mut command = Command::new(&plan.java);
    command.current_dir(&plan.working_dir);
    if plan.uses_argfile() {
        write_argfile(&plan.argfile, plan)?;
        command.arg(argfile_argument(&plan.argfile));
    } else {
        command
            .args(&plan.jvm_args)
            .arg(&plan.main_class)
            .args(&plan.game_args);
    }
    Ok(command)
}

fn argfile_argument(path: &Path) -> std::ffi::OsString {
    let mut argument = std::ffi::OsString::from("@");
    argument.push(path.as_os_str());
    argument
}

fn write_argfile(path: &Path, plan: &LaunchPlan) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).io_at(parent)?;
    }
    let mut body = String::new();
    for arg in plan
        .jvm_args
        .iter()
        .chain(std::iter::once(&plan.main_class))
        .chain(plan.game_args.iter())
    {
        body.push_str(&quote_argfile(arg));
        body.push('\n');
    }
    std::fs::write(path, body).io_at(path)?;
    crate::config::restrict_private(path);
    Ok(())
}

/// Quote one argument the way Java's `@argfile` parser expects.
pub fn quote_argfile(arg: &str) -> String {
    let mut out = String::from("\"");
    for ch in arg.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Quote one argument for a shell-style display.
pub fn quote_shell(arg: &str) -> String {
    if arg.is_empty()
        || arg
            .chars()
            .any(|ch| ch.is_whitespace() || "#\"'\\".contains(ch))
    {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::offline_session;
    use crate::java::JavaInstallation;
    use crate::platform::{Arch, OsName};
    use crate::rules::FeatureSet;
    use crate::version::parse_version;

    fn installation() -> JavaInstallation {
        JavaInstallation {
            path: PathBuf::from("/usr/bin/java"),
            major: 21,
            version: "21.0.2".into(),
            source: "test".into(),
        }
    }

    fn spec_for(version_json: &str, os: OsInfo, features: FeatureSet) -> LaunchSpec {
        let version = parse_version(version_json.as_bytes()).unwrap();
        LaunchSpec {
            version,
            classpath: vec![PathBuf::from("/libs/lwjgl.jar")],
            client_jar: PathBuf::from("/versions/client.jar"),
            natives_dir: PathBuf::from("/natives"),
            game_dir: PathBuf::from("/game dir"),
            assets_dir: PathBuf::from("/assets"),
            assets_index_name: "17".into(),
            game_assets: PathBuf::from("/resources"),
            libraries_dir: PathBuf::from("/libs"),
            log_argument: Some("-Dlog4j.configurationFile=/assets/log.xml".into()),
            session: offline_session("Steve"),
            java: installation(),
            memory: "2G".into(),
            os,
            features,
            client_id: "client-id".into(),
            quick_play_path: String::new(),
            quick_play_singleplayer: String::new(),
            quick_play_multiplayer: String::new(),
            quick_play_realms: String::new(),
            width: None,
            height: None,
            launcher_name: "weathered".into(),
            launcher_version: "0.1.0".into(),
            argfile: PathBuf::from("/tmp/args.txt"),
            java_warning: None,
        }
    }

    const MODERN: &str = r#"{
      "id": "1.21.1",
      "type": "release",
      "mainClass": "net.minecraft.client.main.Main",
      "assets": "17",
      "arguments": {
        "jvm": [
          { "rules": [ { "action": "allow", "os": { "name": "osx" } } ], "value": [ "-XstartOnFirstThread" ] },
          { "rules": [ { "action": "allow", "os": { "name": "windows" } } ], "value": "-XX:HeapDumpPath=heapdump" },
          { "rules": [ { "action": "allow", "os": { "name": "windows", "version": "^10\\." } } ], "value": [ "-Dos.name=Windows 10", "-Dos.version=10.0" ] },
          { "rules": [ { "action": "allow", "os": { "arch": "x86" } } ], "value": "-Xss1M" },
          "-Djava.library.path=${natives_directory}",
          "-Dminecraft.launcher.brand=${launcher_name}",
          "-Dminecraft.launcher.version=${launcher_version}",
          "-cp",
          "${classpath}"
        ],
        "game": [
          "--username", "${auth_player_name}",
          "--version", "${version_name}",
          "--gameDir", "${game_directory}",
          "--assetsDir", "${assets_root}",
          "--assetIndex", "${assets_index_name}",
          "--uuid", "${auth_uuid}",
          "--accessToken", "${auth_access_token}",
          "--userType", "${user_type}",
          "--versionType", "${version_type}",
          { "rules": [ { "action": "allow", "features": { "is_demo_user": true } } ], "value": "--demo" },
          { "rules": [ { "action": "allow", "features": { "has_custom_resolution": true } } ], "value": [ "--width", "${resolution_width}", "--height", "${resolution_height}" ] }
        ]
      }
    }"#;

    #[test]
    fn linux_launch_skips_windows_and_macos_rules() {
        let os = OsInfo::new(OsName::Linux, Arch::X86_64, "6.8.0");
        let plan = build_launch_plan(spec_for(MODERN, os, FeatureSet::default())).unwrap();
        assert_eq!(plan.main_class, "net.minecraft.client.main.Main");
        assert_eq!(plan.jvm_args[0], "-Xmx2G");
        let natives = PathBuf::from("/natives").display().to_string();
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == &format!("-Djava.library.path={natives}")));
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == "-Dminecraft.launcher.brand=weathered"));
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == "-Dminecraft.launcher.version=0.1.0"));
        assert!(plan.jvm_args.iter().any(|arg| arg == "-cp"));
        let classpath = plan
            .jvm_args
            .iter()
            .find(|arg| arg.contains("client.jar"))
            .unwrap();
        let library = PathBuf::from("/libs/lwjgl.jar").display().to_string();
        assert!(classpath.contains(&library));
        assert!(classpath.contains(':'));
        assert!(!classpath.contains(';'));
        assert!(!plan
            .jvm_args
            .iter()
            .any(|arg| arg.contains("XstartOnFirstThread")));
        assert!(!plan.jvm_args.iter().any(|arg| arg.contains("HeapDump")));
        assert!(!plan.jvm_args.iter().any(|arg| arg.contains("Windows 10")));
        assert!(!plan.jvm_args.iter().any(|arg| arg == "-Xss1M"));
        assert!(plan.jvm_args.iter().any(|arg| arg.contains("log4j")));
        let game_dir = PathBuf::from("/game dir").display().to_string();
        assert_eq!(plan.game_args[0], "--username");
        assert_eq!(plan.game_args[1], "Steve");
        assert_eq!(plan.game_args[2], "--version");
        assert_eq!(plan.game_args[3], "1.21.1");
        assert_eq!(plan.game_args[4], "--gameDir");
        assert_eq!(plan.game_args[5], game_dir);
        assert!(plan
            .game_args
            .iter()
            .any(|arg| arg == "5627dd98-e6be-3c21-b8a8-e92344183641"));
        assert!(plan.game_args.iter().any(|arg| arg == "0"));
        assert!(plan.game_args.iter().any(|arg| arg == "legacy"));
        assert!(!plan.game_args.iter().any(|arg| arg == "--demo"));
        assert!(!plan.game_args.iter().any(|arg| arg == "--width"));
    }

    #[test]
    fn windows_10_and_32_bit_rules() {
        let windows_10 = OsInfo::new(OsName::Windows, Arch::X86_64, "10.0.19045.3803");
        let plan = build_launch_plan(spec_for(MODERN, windows_10, FeatureSet::default())).unwrap();
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == "-XX:HeapDumpPath=heapdump"));
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == "-Dos.name=Windows 10"));
        assert!(plan.jvm_args.iter().any(|arg| arg == "-Dos.version=10.0"));
        assert!(!plan.jvm_args.iter().any(|arg| arg == "-Xss1M"));
        let classpath = plan
            .jvm_args
            .iter()
            .find(|arg| arg.contains("client.jar"))
            .unwrap();
        assert!(classpath.contains(';'));

        let windows_7 = OsInfo::new(OsName::Windows, Arch::X86, "6.1.7601");
        let plan = build_launch_plan(spec_for(MODERN, windows_7, FeatureSet::default())).unwrap();
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == "-XX:HeapDumpPath=heapdump"));
        assert!(!plan.jvm_args.iter().any(|arg| arg.contains("Windows 10")));
        assert!(plan.jvm_args.iter().any(|arg| arg == "-Xss1M"));
    }

    #[test]
    fn macos_demo_and_custom_resolution() {
        let features = FeatureSet {
            is_demo_user: true,
            has_custom_resolution: true,
            ..FeatureSet::default()
        };
        let mut spec = spec_for(
            MODERN,
            OsInfo::new(OsName::Osx, Arch::Arm64, "14.5"),
            features,
        );
        spec.width = Some(854);
        spec.height = Some(480);
        let plan = build_launch_plan(spec).unwrap();
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == "-XstartOnFirstThread"));
        assert!(!plan.jvm_args.iter().any(|arg| arg == "-Xss1M"));
        assert!(plan.game_args.iter().any(|arg| arg == "--demo"));
        let width = plan
            .game_args
            .iter()
            .position(|arg| arg == "--width")
            .unwrap();
        assert_eq!(plan.game_args[width + 1], "854");
        assert_eq!(plan.game_args[width + 3], "480");
    }

    #[test]
    fn legacy_arguments_keep_spaces_inside_one_argument() {
        let json = r#"{
          "id": "b1.7.3",
          "type": "old_beta",
          "mainClass": "net.minecraft.launchwrapper.Launch",
          "minecraftArguments": "${auth_player_name} ${auth_session} --gameDir ${game_directory} --assetsDir ${game_assets}"
        }"#;
        let plan = build_launch_plan(spec_for(
            json,
            OsInfo::new(OsName::Linux, Arch::X86_64, ""),
            FeatureSet::default(),
        ))
        .unwrap();
        assert_eq!(plan.game_args[0], "Steve");
        assert!(plan.game_args[1].starts_with("token:0:"));
        assert_eq!(plan.game_args[2], "--gameDir");
        assert_eq!(
            plan.game_args[3],
            PathBuf::from("/game dir").display().to_string()
        );
        assert_eq!(
            plan.game_args[5],
            PathBuf::from("/resources").display().to_string()
        );
        let natives = PathBuf::from("/natives").display().to_string();
        assert!(plan
            .jvm_args
            .iter()
            .any(|arg| arg == &format!("-Djava.library.path={natives}")));
        assert!(plan.jvm_args.iter().any(|arg| arg == "-cp"));
    }

    #[test]
    fn substitution_replaces_a_prefix_and_leaves_unknown_names() {
        let mut vars = HashMap::new();
        vars.insert("natives_directory".into(), "/natives".into());
        assert_eq!(
            substitute("-Djava.library.path=${natives_directory}/lwjgl", &vars),
            "-Djava.library.path=/natives/lwjgl"
        );
        assert_eq!(substitute("${missing}", &vars), "${missing}");
    }

    #[test]
    fn argfile_quoting_escapes_spaces_and_quotes() {
        assert_eq!(quote_argfile("/game dir"), "\"/game dir\"");
        assert_eq!(quote_argfile("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_shell("plain"), "plain");
        assert_eq!(quote_shell("/game dir"), "\"/game dir\"");
    }

    #[test]
    fn display_hides_a_real_access_token() {
        let mut spec = spec_for(
            MODERN,
            OsInfo::new(OsName::Linux, Arch::X86_64, ""),
            FeatureSet::default(),
        );
        spec.session.access_token = "secret-token".into();
        spec.session.user_type = "msa".into();
        let plan = build_launch_plan(spec).unwrap();
        let shown = plan.display_command();
        assert!(!shown.contains("secret-token"));
        assert!(shown.contains("***"));
        assert!(plan.game_args.iter().any(|arg| arg == "secret-token"));
    }

    #[test]
    fn jvm_overrides_insert_a_minimum_heap_and_extra_arguments() {
        let spec = spec_for(
            MODERN,
            OsInfo::new(OsName::Linux, Arch::X86_64, ""),
            FeatureSet::default(),
        );
        let mut plan = build_launch_plan(spec).unwrap();
        apply_jvm_overrides(&mut plan, "512M", "-Dexample=1");
        assert_eq!(plan.jvm_args[0], "-Xms512M");
        assert!(plan.jvm_args.iter().any(|arg| arg == "-Dexample=1"));
        let before = plan.jvm_args.len();
        apply_jvm_overrides(&mut plan, "", "");
        assert_eq!(plan.jvm_args.len(), before);
    }
}
