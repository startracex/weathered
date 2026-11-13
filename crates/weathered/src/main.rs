//! Command-line front end for Weathered.

#![forbid(unsafe_code)]

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use weathered_core::auth::{AccountStore, MicrosoftAuth, Session};
use weathered_core::config::{resolve_ms_client_id, Config};
use weathered_core::download::build_http_client;
use weathered_core::install::{install_version, InstallOptions};
use weathered_core::java::{self, JavaMode};
use weathered_core::launch::{prepare_launch, spawn, LaunchRequest};
use weathered_core::manifest::{self, VersionKind};
use weathered_core::paths::GamePaths;
use weathered_core::platform::OsInfo;
use weathered_core::progress::{ProgressEvent, ProgressSink};
use weathered_core::rules::FeatureSet;
use weathered_core::{Error, LAUNCHER_NAME};

mod tui;

#[derive(Debug, Parser)]
#[command(
    name = "weathered",
    version,
    about = "Weathered Launcher for Minecraft: Java Edition",
    after_help = "Examples:\n  weathered\n  weathered tui\n  weathered list\n  weathered install 1.21.1\n  weathered launch 1.21.1 --username Steve\n  weathered login --client-id <azure-app-id>"
)]
struct Cli {
    /// Game directory. Defaults to $WEATHERED_GAME_DIR or the platform data directory.
    #[arg(long, global = true, value_name = "DIR")]
    game_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List versions from Mojang's manifest.
    List {
        /// Which channel to show. Defaults to releases and snapshots.
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        /// Maximum rows. 0 lists every match.
        #[arg(long, default_value_t = 30)]
        limit: usize,
        /// Case-insensitive substring matched against the version id.
        #[arg(long)]
        search: Option<String>,
    },
    /// Download a version, its libraries, natives, and assets.
    Install {
        /// Version id, or `release` / `snapshot` for the latest of that channel.
        version: String,
        /// Do not download a Java runtime.
        #[arg(long)]
        no_java: bool,
        /// Download Mojang's Java runtime even when a matching JDK is already installed.
        #[arg(long, conflicts_with = "no_java")]
        download_java: bool,
        /// Concurrent downloads.
        #[arg(long, default_value_t = 32, value_name = "N")]
        jobs: usize,
    },
    /// Launch an installed version.
    Launch(Box<LaunchArgs>),
    /// Sign in with a Microsoft account using the device-code flow.
    Login {
        /// Azure application (client) id. Saved for later logins.
        #[arg(long)]
        client_id: Option<String>,
    },
    /// List or remove saved Microsoft accounts.
    Accounts {
        #[command(subcommand)]
        action: AccountsCmd,
    },
    /// Show Java runtimes Weathered can see.
    Java,
    /// Open the full-screen terminal UI. This is also the default when no command is given.
    Tui,
}

#[derive(Debug, clap::Args)]
struct LaunchArgs {
    /// Version id, or `release` / `snapshot`.
    version: String,
    /// Offline username. Omit this to use a saved Microsoft account.
    #[arg(long, conflicts_with = "account")]
    username: Option<String>,
    /// Saved Microsoft account, by username or UUID.
    #[arg(long)]
    account: Option<String>,
    /// Java binary to use instead of the detected runtime.
    #[arg(long, value_name = "PATH")]
    java: Option<PathBuf>,
    /// Maximum heap size passed as -Xmx.
    #[arg(long, default_value = "2G")]
    memory: String,
    /// Launch the demo world.
    #[arg(long)]
    demo: bool,
    /// Window width. Requires --height.
    #[arg(long)]
    width: Option<u32>,
    /// Window height. Requires --width.
    #[arg(long)]
    height: Option<u32>,
    /// Quick Play singleplayer world name.
    #[arg(long)]
    quick_play_singleplayer: Option<String>,
    /// Quick Play multiplayer address.
    #[arg(long)]
    quick_play_multiplayer: Option<String>,
    /// Quick Play path.
    #[arg(long)]
    quick_play_path: Option<String>,
    /// Quick Play realm id.
    #[arg(long)]
    quick_play_realms: Option<String>,
    /// Print the command and do not start the game.
    #[arg(long)]
    dry_run: bool,
    /// Do not download a Java runtime if one is missing.
    #[arg(long)]
    no_java: bool,
    /// Print the launch command before starting the game.
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Subcommand)]
enum AccountsCmd {
    /// List saved accounts.
    List,
    /// Remove an account by username or UUID.
    Remove {
        /// Username or UUID.
        id: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum KindArg {
    /// Stable releases.
    Release,
    /// Snapshots.
    Snapshot,
    /// Old beta versions.
    #[value(name = "old-beta")]
    OldBeta,
    /// Old alpha versions.
    #[value(name = "old-alpha")]
    OldAlpha,
    /// Every channel, including old alpha and beta.
    All,
}

impl KindArg {
    fn kinds(self) -> Option<Vec<VersionKind>> {
        match self {
            Self::Release => Some(vec![VersionKind::Release]),
            Self::Snapshot => Some(vec![VersionKind::Snapshot]),
            Self::OldBeta => Some(vec![VersionKind::OldBeta]),
            Self::OldAlpha => Some(vec![VersionKind::OldAlpha]),
            Self::All => None,
        }
    }
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    let cli = Cli::parse();
    let command = match cli.command {
        None | Some(Command::Tui) => return tui::run(cli.game_dir).await,
        Some(command) => command,
    };
    let paths = GamePaths::new(game_dir(cli.game_dir));
    match command {
        Command::Tui => unreachable!(),
        Command::List {
            kind,
            limit,
            search,
        } => cmd_list(&paths, kind, limit, search.as_deref()).await,
        Command::Install {
            version,
            no_java,
            download_java,
            jobs,
        } => {
            cmd_install(&paths, &version, no_java, download_java, jobs).await?;
            Ok(0)
        }
        Command::Launch(args) => {
            cmd_launch(LaunchCli {
                paths: &paths,
                version: args.version,
                username: args.username,
                account: args.account,
                java: args.java,
                memory: args.memory,
                demo: args.demo,
                width: args.width,
                height: args.height,
                quick_play_singleplayer: args.quick_play_singleplayer,
                quick_play_multiplayer: args.quick_play_multiplayer,
                quick_play_path: args.quick_play_path,
                quick_play_realms: args.quick_play_realms,
                dry_run: args.dry_run,
                no_java: args.no_java,
                verbose: args.verbose,
            })
            .await
        }
        Command::Login { client_id } => {
            cmd_login(client_id.as_deref()).await?;
            Ok(0)
        }
        Command::Accounts { action } => {
            cmd_accounts(action)?;
            Ok(0)
        }
        Command::Java => {
            cmd_java(&paths);
            Ok(0)
        }
    }
}

fn game_dir(flag: Option<PathBuf>) -> PathBuf {
    flag.unwrap_or_else(GamePaths::default_root)
}

async fn cmd_list(
    paths: &GamePaths,
    kind: Option<KindArg>,
    limit: usize,
    search: Option<&str>,
) -> Result<i32> {
    let client = build_http_client()?;
    let fetched = manifest::fetch_manifest(&client, &paths.manifest_cache()).await?;
    if fetched.from_cache {
        eprintln!("warning: using the cached version manifest because the network request failed");
    }
    let manifest = fetched.manifest;
    println!("Latest release:  {}", manifest.latest.release);
    println!("Latest snapshot: {}", manifest.latest.snapshot);
    println!();

    let kinds = match kind {
        None => Some(vec![VersionKind::Release, VersionKind::Snapshot]),
        Some(KindArg::All) => None,
        Some(other) => other.kinds(),
    };
    let matches = manifest.query(kinds.as_deref(), search);
    let shown = if limit == 0 {
        matches.len()
    } else {
        limit.min(matches.len())
    };
    println!("{:<24} {:<10} RELEASED", "ID", "TYPE");
    for entry in matches.iter().take(shown) {
        let released = entry
            .release_time
            .as_deref()
            .and_then(|time| time.split('T').next())
            .unwrap_or("-");
        println!("{:<24} {:<10} {}", entry.id, entry.version_type, released);
    }
    if shown < matches.len() {
        println!();
        println!(
            "Showing {shown} of {}. Pass --limit 0 to list every match.",
            matches.len()
        );
    }
    Ok(0)
}

async fn cmd_install(
    paths: &GamePaths,
    version: &str,
    no_java: bool,
    download_java: bool,
    jobs: usize,
) -> Result<()> {
    let java_mode = if no_java {
        JavaMode::Skip
    } else if download_java {
        JavaMode::ForceDownload
    } else {
        JavaMode::Auto
    };
    println!("Installing {version} into {}", paths.root.display());
    let client = build_http_client()?;
    let report = install_version(
        &client,
        paths,
        &InstallOptions {
            version: version.to_string(),
            os: OsInfo::current(),
            concurrency: jobs,
            java: java_mode,
        },
        &cli_progress(),
    )
    .await?;
    if report.used_cached_manifest {
        eprintln!("warning: the version manifest came from the local cache");
    }
    println!();
    println!("Installed {} ({})", report.version_id, report.version_type);
    println!("  directory: {}", paths.root.display());
    println!("  client:    {}", report.client_jar.display());
    println!("  libraries: {} on the classpath", report.libraries);
    println!("  natives:   {} extracted", report.natives);
    println!("  assets:    {}", report.assets);
    println!(
        "  files:     {} downloaded, {} already valid",
        report.files_downloaded, report.files_cached
    );
    println!("  java:      {}", report.java_note);
    Ok(())
}

struct LaunchCli<'a> {
    paths: &'a GamePaths,
    version: String,
    username: Option<String>,
    account: Option<String>,
    java: Option<PathBuf>,
    memory: String,
    demo: bool,
    width: Option<u32>,
    height: Option<u32>,
    quick_play_singleplayer: Option<String>,
    quick_play_multiplayer: Option<String>,
    quick_play_path: Option<String>,
    quick_play_realms: Option<String>,
    dry_run: bool,
    no_java: bool,
    verbose: bool,
}

async fn cmd_launch(args: LaunchCli<'_>) -> Result<i32> {
    if !valid_memory(&args.memory) {
        bail!("--memory must look like 2G, 4096M, or 512m");
    }
    match (args.width, args.height) {
        (None, None) | (Some(_), Some(_)) => {}
        _ => bail!("pass both --width and --height, or neither"),
    }
    let mut config = Config::load()?;
    let client_id = config.ensure_client_id();
    let session = resolve_session(
        args.username.as_deref(),
        args.account.as_deref(),
        &client_id,
    )
    .await?;

    let features = FeatureSet {
        is_demo_user: args.demo,
        has_custom_resolution: args.width.is_some(),
        has_quick_plays_support: args.quick_play_path.is_some(),
        is_quick_play_singleplayer: args.quick_play_singleplayer.is_some(),
        is_quick_play_multiplayer: args.quick_play_multiplayer.is_some(),
        is_quick_play_realms: args.quick_play_realms.is_some(),
    };

    let client = build_http_client()?;
    let plan = prepare_launch(
        &client,
        args.paths,
        LaunchRequest {
            version_id: args.version,
            session,
            java: args.java,
            memory: args.memory,
            os: OsInfo::current(),
            features,
            client_id,
            quick_play_path: args.quick_play_path.unwrap_or_default(),
            quick_play_singleplayer: args.quick_play_singleplayer.unwrap_or_default(),
            quick_play_multiplayer: args.quick_play_multiplayer.unwrap_or_default(),
            quick_play_realms: args.quick_play_realms.unwrap_or_default(),
            width: args.width,
            height: args.height,
            java_mode: if args.no_java {
                JavaMode::Skip
            } else {
                JavaMode::Auto
            },
            concurrency: 32,
        },
        &cli_progress(),
    )
    .await?;

    if let Some(warning) = &plan.java_warning {
        eprintln!("warning: {warning}");
    }
    println!("Launching {} as {}", plan.version_id, plan.username);
    println!(
        "Java: {} ({}, {})",
        plan.java.display(),
        plan.java_version,
        plan.java_source
    );
    println!("Game directory: {}", plan.working_dir.display());
    if args.dry_run || args.verbose {
        println!("{}", plan.display_command());
    }
    if args.dry_run {
        println!("Dry run: not starting the game.");
        return Ok(0);
    }
    let mut child = spawn(&plan)?;
    let status = child.wait().context("failed while waiting for Minecraft")?;
    Ok(status.code().unwrap_or(1))
}

async fn resolve_session(
    username: Option<&str>,
    account: Option<&str>,
    _client_id: &str,
) -> Result<Session> {
    if let Some(username) = username {
        let username = username.trim();
        if username.is_empty() || username.chars().any(|ch| ch.is_control()) {
            bail!("--username must not be empty or contain control characters");
        }
        return Ok(weathered_core::auth::offline_session(username));
    }

    let mut store = AccountStore::load()?;
    let mut saved = if let Some(query) = account {
        store
            .get(query)
            .cloned()
            .ok_or_else(|| anyhow!("no saved account matches {query}"))?
    } else if let Some(active) = store.active_account() {
        active.clone()
    } else {
        match store.accounts.as_slice() {
            [only] => only.clone(),
            [] => bail!(
                "no Microsoft account is saved. Pass --username for offline play, or run `{LAUNCHER_NAME} login`"
            ),
            many => {
                let names = many
                    .iter()
                    .map(|account| account.username.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("several accounts are saved ({names}). Pass --account to choose one");
            }
        }
    };

    if saved.is_expired() {
        let config = Config::load()?;
        let env = std::env::var("WEATHERED_MS_CLIENT_ID").ok();
        let Some(ms_client_id) = resolve_ms_client_id(None, env.as_deref(), &config) else {
            bail!(
                "the saved Minecraft token has expired and no Microsoft client id is configured.\n{}",
                weathered_core::auth::microsoft::microsoft_setup_help(&Config::path())
            );
        };
        eprintln!("Refreshing the Minecraft token for {}...", saved.username);
        let client = build_http_client()?;
        let auth = MicrosoftAuth::new(client, ms_client_id)?;
        saved = auth.refresh(&saved).await?;
        let uuid = saved.uuid.clone();
        store.upsert(saved.clone());
        store.set_active(&uuid);
        store.save()?;
    }
    Ok(saved.session())
}

async fn cmd_login(client_id: Option<&str>) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(client_id) = client_id.map(str::trim).filter(|id| !id.is_empty()) {
        config.ms_client_id = client_id.to_string();
        config.save()?;
    }
    let env = std::env::var("WEATHERED_MS_CLIENT_ID").ok();
    let Some(ms_client_id) = resolve_ms_client_id(client_id, env.as_deref(), &config) else {
        eprintln!(
            "{}",
            weathered_core::auth::microsoft::microsoft_setup_help(&Config::path())
        );
        return Err(Error::MissingClientId.into());
    };

    let client = build_http_client()?;
    let auth = MicrosoftAuth::new(client, ms_client_id)?;
    println!("Starting Microsoft device-code login.");
    let account = auth
        .login(|code| {
            if let Some(message) = &code.message {
                println!("{message}");
            } else {
                println!(
                    "Open {} and enter code {}",
                    code.verification_uri, code.user_code
                );
            }
            println!("Waiting for authorization...");
        })
        .await?;

    let mut store = AccountStore::load()?;
    let uuid = account.uuid.clone();
    let username = account.username.clone();
    store.upsert(account);
    store.set_active(&uuid);
    store.save()?;
    println!("Signed in as {username} ({uuid})");
    Ok(())
}

fn cmd_accounts(action: AccountsCmd) -> Result<()> {
    let mut store = AccountStore::load()?;
    match action {
        AccountsCmd::List => {
            if store.accounts.is_empty() {
                println!("No Microsoft accounts saved.");
                return Ok(());
            }
            for account in &store.accounts {
                let marker = if store.active.as_deref() == Some(account.uuid.as_str()) {
                    "*"
                } else {
                    " "
                };
                let state = if account.is_expired() {
                    "expired"
                } else {
                    "valid"
                };
                println!(
                    "{marker} {:<16} {}  {state}",
                    account.username, account.uuid
                );
            }
        }
        AccountsCmd::Remove { id } => {
            if !store.remove(&id) {
                bail!("no saved account matches {id}");
            }
            store.save()?;
            println!("Removed {id}");
        }
    }
    Ok(())
}

fn cmd_java(paths: &GamePaths) {
    println!("System runtimes");
    let system = java::detect_system_javas();
    if system.is_empty() {
        println!("  (none found)");
    } else {
        for installation in &system {
            println!(
                "  Java {:<8} {}  ({})",
                installation.version,
                installation.path.display(),
                installation.source
            );
        }
    }
    println!();
    println!("Mojang runtimes in {}", paths.root.display());
    let vendored = java::detect_vendored_runtimes(paths);
    if vendored.is_empty() {
        println!("  (none)");
    } else {
        for installation in &vendored {
            println!(
                "  Java {:<8} {}  ({})",
                installation.version,
                installation.path.display(),
                installation.source
            );
        }
    }
}

fn valid_memory(memory: &str) -> bool {
    let memory = memory.trim();
    let digits = memory.trim_end_matches(['k', 'K', 'm', 'M', 'g', 'G']);
    !digits.is_empty()
        && digits.chars().all(|ch| ch.is_ascii_digit())
        && memory.len() - digits.len() <= 1
}

fn cli_progress() -> ProgressSink {
    let last = Mutex::new(None::<(String, u64, Instant)>);
    let tty = io::stderr().is_terminal();
    ProgressSink::new(move |event: ProgressEvent| {
        let mut last = last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let stage_changed = last
            .as_ref()
            .map(|(stage, _, _)| stage != &event.stage)
            .unwrap_or(true);
        let due = last
            .as_ref()
            .map(|(_, _, when)| when.elapsed().as_millis() >= 200)
            .unwrap_or(true);
        let milestone = event.completed == event.total
            || event.completed == 1
            || event.completed.is_multiple_of(50);
        if !tty && !stage_changed && !milestone {
            return;
        }
        if tty && !stage_changed && !due && event.completed != event.total {
            return;
        }
        *last = Some((event.stage.clone(), event.completed, Instant::now()));
        let detail = shorten(&event.detail, 72);
        if tty {
            let _ = write!(
                io::stderr(),
                "\r\x1b[2K{:<12} {:>5}/{}  {}",
                event.stage,
                event.completed,
                event.total,
                detail
            );
            if event.completed == event.total {
                let _ = writeln!(io::stderr());
            }
            let _ = io::stderr().flush();
        } else {
            eprintln!(
                "{:<12} {:>5}/{}  {}",
                event.stage, event.completed, event.total, detail
            );
        }
    })
}

fn shorten(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    let clipped: String = text.chars().take(keep).collect();
    format!("{clipped}...")
}
