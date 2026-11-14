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
use weathered_core::content::{self, Loader};
use weathered_core::download::build_http_client;
use weathered_core::instance::{self};
use weathered_core::java::{self, JavaMode};
use weathered_core::launch::{prepare_launch, spawn, LaunchRequest};
use weathered_core::manifest::{self, VersionKind};
use weathered_core::paths::{DataLayout, GamePaths};
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
    after_help = "Examples:\n  weathered\n  weathered download 1.21.1\n  weathered download 1.21.1 --loader fabric\n  weathered instance create --name Skyblock 1.21.1 --loader fabric\n  weathered instance import pack.mrpack\n  weathered launch Skyblock --username Steve"
)]
struct Cli {
    /// Data directory. Holds the store, instances, and any legacy game files.
    /// Defaults to $WEATHERED_GAME_DIR or the platform data directory.
    #[arg(long, global = true, value_name = "DIR")]
    game_dir: Option<PathBuf>,

    /// Concurrent downloads. Defaults to 16, or download_jobs in the config.
    #[arg(long, global = true, value_name = "N")]
    jobs: Option<usize>,

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
    /// Download a version into the shared store. Does not create an instance.
    Install {
        /// Version id, or `release` / `snapshot`. For Forge or NeoForge, the loader version.
        version: String,
        /// `vanilla`, `fabric`, `quilt`, `forge`, or `neoforge`.
        #[arg(long, default_value = "vanilla")]
        loader: String,
        /// Loader build. Fabric and Quilt default to the newest stable build.
        #[arg(long)]
        loader_version: Option<String>,
        /// Do not download a Java runtime.
        #[arg(long)]
        no_java: bool,
        /// Download Mojang's Java runtime even when a matching JDK is already installed.
        #[arg(long, conflicts_with = "no_java")]
        download_java: bool,
    },
    /// Alias of install.
    Download {
        /// Version id, or a Forge/NeoForge version.
        version: String,
        /// `vanilla`, `fabric`, `quilt`, `forge`, or `neoforge`.
        #[arg(long, default_value = "vanilla")]
        loader: String,
        /// Loader build. Fabric and Quilt default to the newest stable build.
        #[arg(long)]
        loader_version: Option<String>,
        /// Do not download a Java runtime.
        #[arg(long)]
        no_java: bool,
        /// Download Mojang's Java runtime even when a matching JDK is already installed.
        #[arg(long, conflicts_with = "no_java")]
        download_java: bool,
    },
    /// Create, list, import, or delete instances.
    Instance {
        #[command(subcommand)]
        action: InstanceCmd,
    },
    /// Launch an instance. A legacy version id still launches the old game directory.
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
    /// Instance name or id. A version id launches a legacy install when no instance matches.
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
enum InstanceCmd {
    /// List instances.
    List,
    /// Create an instance from the store, downloading the profile first if needed.
    Create {
        /// Display name.
        #[arg(long)]
        name: String,
        /// Catalog id or Minecraft version.
        version: String,
        /// `vanilla`, `fabric`, or `quilt`.
        #[arg(long, default_value = "vanilla")]
        loader: String,
        /// Loader build. Defaults to the newest stable Fabric or Quilt loader.
        #[arg(long)]
        loader_version: Option<String>,
        /// Heap override stored on the instance, such as `4G`.
        #[arg(long)]
        memory: Option<String>,
        /// Java binary stored on the instance.
        #[arg(long)]
        java: Option<PathBuf>,
    },
    /// Import a Modrinth `.mrpack`.
    Import {
        /// Path to the `.mrpack` file.
        pack: PathBuf,
        /// Instance name. Defaults to the pack name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Delete an instance. A legacy instance only removes its metadata.
    Delete {
        /// Instance name or id.
        instance: String,
    },
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
    let paths = GamePaths::new(game_dir(cli.game_dir.clone()));
    let layout = DataLayout::new(paths.root.clone());
    let jobs = cli
        .jobs
        .unwrap_or_else(|| Config::load().map(|config| config.jobs()).unwrap_or(16));
    match command {
        Command::Tui => unreachable!(),
        Command::List {
            kind,
            limit,
            search,
        } => cmd_list(&paths, kind, limit, search.as_deref()).await,
        Command::Install {
            version,
            loader,
            loader_version,
            no_java,
            download_java,
        }
        | Command::Download {
            version,
            loader,
            loader_version,
            no_java,
            download_java,
        } => {
            cmd_download(
                &layout,
                &version,
                &loader,
                loader_version.as_deref(),
                no_java,
                download_java,
                jobs,
            )
            .await?;
            Ok(0)
        }
        Command::Instance { action } => {
            cmd_instance(&layout, action, jobs).await?;
            Ok(0)
        }
        Command::Launch(args) => {
            cmd_launch(LaunchCli {
                paths: &paths,
                layout: &layout,
                jobs,
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

async fn cmd_download(
    layout: &DataLayout,
    version: &str,
    loader: &str,
    loader_version: Option<&str>,
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
    let parsed = Loader::parse(loader)?;
    println!(
        "Downloading {version} ({}) into {}",
        parsed.as_str(),
        layout.store_root().display()
    );
    let client = build_http_client()?;
    let os = OsInfo::current();
    let item = content::download(
        &client,
        layout,
        content::DownloadRequest {
            version,
            loader: parsed,
            loader_version,
            os: &os,
            concurrency: jobs,
            java_mode,
        },
        &cli_progress(),
    )
    .await?;
    println!();
    println!("Stored {} ({})", item.name, item.id);
    if item.launchable {
        println!(
            "Create an instance with `{LAUNCHER_NAME} instance create --name <name> {}`",
            item.id
        );
    } else {
        println!("{}", item.note);
    }
    Ok(())
}

async fn download_for_instance(
    client: &reqwest::Client,
    layout: &DataLayout,
    version: &str,
    loader: Loader,
    loader_version: Option<&str>,
    jobs: usize,
) -> Result<weathered_core::catalog::CatalogItem> {
    let os = OsInfo::current();
    content::download(
        client,
        layout,
        content::DownloadRequest {
            version,
            loader,
            loader_version,
            os: &os,
            concurrency: jobs,
            java_mode: JavaMode::Auto,
        },
        &cli_progress(),
    )
    .await
    .map_err(Into::into)
}

async fn cmd_instance(layout: &DataLayout, action: InstanceCmd, jobs: usize) -> Result<()> {
    let _ = instance::migrate_legacy(layout)?;
    match action {
        InstanceCmd::List => {
            let items = instance::list(layout)?;
            if items.is_empty() {
                println!(
                    "No instances. Download a version, then run `{LAUNCHER_NAME} instance create`."
                );
                return Ok(());
            }
            println!("{:<16} {:<16} {:<10} LOADER", "ID", "NAME", "VERSION");
            for item in items {
                let loader = if item.loader_version.is_empty() {
                    item.loader.clone()
                } else {
                    format!("{} {}", item.loader, item.loader_version)
                };
                println!(
                    "{:<16} {:<16} {:<10} {loader}",
                    item.id, item.name, item.game_version
                );
            }
        }
        InstanceCmd::Create {
            name,
            version,
            loader,
            loader_version,
            memory,
            java,
        } => {
            let parsed = Loader::parse(&loader)?;
            let client = build_http_client()?;
            let item = if let Some(existing) = weathered_core::catalog::find(layout, &version)? {
                if existing.kind == parsed.as_str()
                    || (parsed == Loader::Vanilla && existing.id == version)
                {
                    existing
                } else {
                    download_for_instance(
                        &client,
                        layout,
                        &version,
                        parsed,
                        loader_version.as_deref(),
                        jobs,
                    )
                    .await?
                }
            } else {
                download_for_instance(
                    &client,
                    layout,
                    &version,
                    parsed,
                    loader_version.as_deref(),
                    jobs,
                )
                .await?
            };
            let instance = instance::create(
                layout,
                &name,
                &item.id,
                memory.as_deref().unwrap_or(""),
                java.as_deref()
                    .map(|path| path.display().to_string())
                    .as_deref()
                    .unwrap_or(""),
            )?;
            println!(
                "Created instance {} ({}) at {}",
                instance.name, instance.id, instance.game_dir
            );
        }
        InstanceCmd::Import { pack, name } => {
            let client = build_http_client()?;
            let instance = instance::import_mrpack(
                &client,
                layout,
                &pack,
                name.as_deref(),
                &OsInfo::current(),
                jobs,
                &cli_progress(),
            )
            .await?;
            println!(
                "Imported {} ({}) for {} {}",
                instance.name, instance.id, instance.loader, instance.game_version
            );
        }
        InstanceCmd::Delete { instance } => {
            let removed = instance::delete(layout, &instance)?;
            println!("Deleted instance {} ({})", removed.name, removed.id);
        }
    }
    Ok(())
}

struct LaunchCli<'a> {
    paths: &'a GamePaths,
    layout: &'a DataLayout,
    jobs: usize,
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
    let _ = instance::migrate_legacy(args.layout)?;
    let request = LaunchRequest {
        version_id: args.version.clone(),
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
        concurrency: args.jobs,
    };
    let plan = if instance::find(args.layout, &args.version)?.is_some() {
        instance::prepare_launch(
            &client,
            args.layout,
            &args.version,
            request,
            &cli_progress(),
        )
        .await?
    } else {
        prepare_launch(&client, args.paths, request, &cli_progress()).await?
    };

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
    let last = Mutex::new(None::<Instant>);
    let tty = io::stderr().is_terminal();
    ProgressSink::new(move |event: ProgressEvent| {
        let mut last = last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let due = last
            .map(|when| when.elapsed().as_millis() >= 100)
            .unwrap_or(true);
        let finished = event.total > 0 && event.completed >= event.total;
        if !finished && !due {
            return;
        }
        *last = Some(Instant::now());
        let line = format_bar(&event);
        if tty {
            let _ = write!(io::stderr(), "\r\x1b[2K{line}");
            if finished {
                let _ = writeln!(io::stderr());
            }
            let _ = io::stderr().flush();
        } else if finished || event.completed == 0 || event.completed.is_multiple_of(1024 * 1024) {
            eprintln!("{line}");
        }
    })
}

fn format_bar(event: &ProgressEvent) -> String {
    let percent = event
        .completed
        .saturating_mul(100)
        .checked_div(event.total)
        .unwrap_or(0)
        .min(100);
    let width = 24usize;
    let filled = (width as u64 * percent / 100) as usize;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    let detail = shorten(&event.detail, 48);
    format!(
        "[{bar}] {percent:>3}%  {}/{}  {detail}",
        format_bytes(event.completed),
        format_bytes(event.total)
    )
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
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
