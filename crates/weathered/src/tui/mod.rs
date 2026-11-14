//! Full-screen terminal UI.
//!
//! `weathered` with no subcommand, and `weathered tui`, open this screen. Installs
//! and launches run on background tasks; the screen keeps reading the keyboard
//! and the mouse while progress events arrive.

mod model;
mod theme;
mod view;

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute, terminal};
use futures::StreamExt;
use model::{App, DetectedJava, Effect, Key, Mouse, Msg, Profile};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use weathered_core::auth::{AccountStore, MicrosoftAuth};
use weathered_core::cancel::CancelFlag;
use weathered_core::config::{Config, resolve_ms_client_id};
use weathered_core::content::{self, Loader};
use weathered_core::download::build_http_client;
use weathered_core::instance;
use weathered_core::java::{self, JavaMode};
use weathered_core::launch::{LaunchRequest, spawn_captured};
use weathered_core::manifest;
use weathered_core::paths::DataLayout;
use weathered_core::paths::GamePaths;
use weathered_core::platform::OsInfo;
use weathered_core::progress::ProgressSink;
use weathered_core::rules::FeatureSet;

/// Open the terminal UI. `game_dir` overrides the saved directory for this run.
pub async fn run(game_dir: Option<PathBuf>) -> Result<i32> {
    if !io::stdout().is_terminal() {
        anyhow::bail!("the terminal UI needs an interactive terminal. Try `weathered --help`.");
    }
    let mut config = Config::load()?;
    let _ = config.ensure_client_id();
    let dir = game_dir.unwrap_or_else(|| {
        if config.game_dir.trim().is_empty() {
            GamePaths::default_root()
        } else {
            PathBuf::from(config.game_dir.trim())
        }
    });
    let mut app = App::empty(dir);
    if !config.memory.trim().is_empty() {
        app.memory = config.memory.trim().to_string();
    }
    app.memory_min = config.memory_min.trim().to_string();
    app.jvm_args = config.jvm_args.trim().to_string();
    app.default_instance = config.default_instance.trim().to_string();
    app.sort_mode = crate::tui::model::SortMode::parse(&config.instance_sort);
    app.download_sort = crate::tui::model::DownloadSort::parse(&config.download_sort);
    if !config.offline_username.trim().is_empty() {
        app.username = config.offline_username.trim().to_string();
    }
    app.java_path = config.java_path.trim().to_string();
    app.client_id_input = config.ms_client_id.trim().to_string();
    app.sync_profiles_from_disk();
    app.reload_installed();
    app.reload_local();
    app.note("Loading version list...", false);

    let (tx, mut rx) = mpsc::channel::<Msg>(512);
    spawn_catalog(tx.clone(), app.game_dir.clone());
    spawn_javas(tx.clone(), app.game_dir.clone());

    let mut guard = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut launch_control: Option<LaunchControl> = None;
    let mut login_task: Option<JoinHandle<()>> = None;
    weathered_core::store::DownloadSlots::install(config.jobs());
    let mut download_tasks: Vec<DownloadTask> = Vec::new();
    let mut picker_task: Option<DownloadTask> = None;

    loop {
        guard.terminal.draw(|frame| view::draw(frame, &mut app))?;
        if app.quit {
            break;
        }
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) => {
                        if let Some(mapped) = map_key(key) {
                            let effect = app.on_key(mapped);
                            dispatch(&mut app, effect, &mut config, &tx, &mut launch_control, &mut login_task, &mut download_tasks, &mut picker_task).await;
                        }
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        if let Some(mapped) = map_mouse(mouse) {
                            let effect = app.on_mouse(mapped);
                            dispatch(&mut app, effect, &mut config, &tx, &mut launch_control, &mut login_task, &mut download_tasks, &mut picker_task).await;
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Err(_)) | None => break,
                    _ => {}
                }
            }
            message = rx.recv() => {
                if let Some(message) = message {
                    let finished_login = matches!(message, Msg::LoginFinished { ok: true, .. });
                    app.on_msg(message);
                    if finished_login {
                        app.sync_profiles_from_disk();
                    }
                }
            }
        }
    }
    if let Some(task) = login_task.take() {
        task.abort();
    }
    stop_all_downloads(&mut download_tasks);
    stop_download(&mut picker_task);
    if let Some(control) = launch_control.take() {
        control.cancel();
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    app: &mut App,
    effect: Effect,
    config: &mut Config,
    tx: &mpsc::Sender<Msg>,
    launch_control: &mut Option<LaunchControl>,
    login_task: &mut Option<JoinHandle<()>>,
    download_tasks: &mut Vec<DownloadTask>,
    picker_task: &mut Option<DownloadTask>,
) {
    match effect {
        Effect::None => {}
        Effect::Quit => {
            stop_all_downloads(download_tasks);
            stop_download(picker_task);
            app.quit = true;
        }
        Effect::CancelJob { id } => {
            stop_one_download(download_tasks, id);
        }
        Effect::Save => {
            if let Err(err) = persist(app, config) {
                app.note(format!("Could not save settings: {err}"), true);
            }
        }
        Effect::RefreshManifest => {
            app.catalog_ready = false;
            app.note("Loading version list...", false);
            spawn_catalog(tx.clone(), app.game_dir.clone());
        }
        Effect::Install {
            id,
            version,
            loader,
            loader_version,
            create_instance,
        } => {
            let cancel = CancelFlag::new();
            let handle = spawn_install(
                tx.clone(),
                app.game_dir.clone(),
                id,
                version,
                loader,
                loader_version,
                create_instance,
                config.jobs(),
                cancel.clone(),
            );
            download_tasks.push(DownloadTask { id, cancel, handle });
        }
        Effect::OpenPicker {
            version,
            loader,
            create_instance,
        } => {
            stop_download(picker_task);
            let cancel = CancelFlag::new();
            let handle =
                spawn_loader_list(tx.clone(), version, loader, create_instance, cancel.clone());
            *picker_task = Some(DownloadTask {
                id: 0,
                cancel,
                handle,
            });
        }
        Effect::ClosePicker => {
            stop_download(picker_task);
        }
        Effect::DeleteInstance => {
            let layout = DataLayout::new(&app.game_dir);
            let id = app
                .draft
                .as_ref()
                .map(|draft| draft.id.clone())
                .or_else(|| app.selected_instance().map(|item| item.id.clone()))
                .unwrap_or_default();
            match instance::delete(&layout, &id) {
                Ok(instance) => {
                    if app.default_instance == instance.id {
                        app.default_instance.clear();
                        let _ = persist(app, config);
                    }
                    app.draft = None;
                    app.note(format!("Deleted {}", instance.name), false);
                }
                Err(err) => app.note(err.to_string(), true),
            }
            app.reload_local();
        }
        Effect::SetDefault => {
            let id = app
                .draft
                .as_ref()
                .map(|draft| draft.id.clone())
                .unwrap_or_default();
            if id.is_empty() {
                app.note("Select an instance first", true);
            } else if app.default_instance == id {
                app.default_instance.clear();
                app.note("Default instance cleared", false);
                if let Err(err) = persist(app, config) {
                    app.note(format!("Could not save settings: {err}"), true);
                }
                app.reload_local();
            } else {
                app.default_instance = id;
                app.note("Default instance set", false);
                if let Err(err) = persist(app, config) {
                    app.note(format!("Could not save settings: {err}"), true);
                }
                app.reload_local();
            }
        }
        Effect::OpenDataDir => match instance::open_in_file_manager(&app.game_dir) {
            Ok(()) => app.note("Opened the data directory", false),
            Err(err) => app.note(err.to_string(), true),
        },
        Effect::OpenFolder => {
            let layout = DataLayout::new(&app.game_dir);
            let id = app
                .draft
                .as_ref()
                .map(|draft| draft.id.clone())
                .unwrap_or_default();
            match instance::find(&layout, &id) {
                Ok(Some(instance)) => {
                    if let Err(err) =
                        instance::open_in_file_manager(std::path::Path::new(&instance.game_dir))
                    {
                        app.note(err.to_string(), true);
                    } else {
                        app.note("Opened the instance folder", false);
                    }
                }
                Ok(None) => app.note("That instance is no longer on disk", true),
                Err(err) => app.note(err.to_string(), true),
            }
        }
        Effect::PersistDraft => {
            let layout = DataLayout::new(&app.game_dir);
            let Some(draft) = app.draft.clone() else {
                return;
            };
            match instance::save_settings(
                &layout,
                &draft.id,
                &draft.name,
                &draft.description,
                &draft.java,
                &draft.memory_min,
                &draft.memory_max,
                &draft.jvm_args,
            ) {
                Ok(instance) => {
                    app.note(format!("Saved {}", instance.name), false);
                    app.reload_local();
                }
                Err(err) => app.note(err.to_string(), true),
            }
        }
        Effect::CreateInstance { catalog_id } => {
            spawn_create(tx.clone(), app.game_dir.clone(), catalog_id);
        }
        Effect::ImportPack { id, path } => {
            let cancel = CancelFlag::new();
            let handle = spawn_import(
                tx.clone(),
                app.game_dir.clone(),
                id,
                path,
                config.jobs(),
                cancel.clone(),
            );
            download_tasks.push(DownloadTask { id, cancel, handle });
        }
        Effect::Launch => {
            if let Some(previous) = launch_control.take() {
                previous.cancel();
            }
            let control = LaunchControl::new();
            let offline = app.selected_profile().is_none_or(|profile| profile.offline);
            let account_uuid = app
                .selected_profile()
                .filter(|profile| !profile.offline)
                .map(|profile| profile.account_uuid.clone())
                .unwrap_or_default();
            let instance_id = app
                .selected_instance()
                .map(|item| item.id.clone())
                .unwrap_or_default();
            let launch_id = app
                .jobs
                .iter()
                .rev()
                .find(|job| job.launching)
                .map(|job| job.id)
                .unwrap_or(0);
            spawn_launch(
                tx.clone(),
                LaunchParams {
                    instance_id,
                    game_dir: app.game_dir.clone(),
                    memory: app.memory.clone(),
                    username: app.username.clone(),
                    java_path: app.java_path.clone(),
                    offline,
                    account_uuid,
                },
                control.clone(),
                launch_id,
            );
            *launch_control = Some(control);
        }
        Effect::StopGame => {
            if let Some(control) = launch_control.take() {
                control.cancel();
            }
            app.mark_game_stopped();
            app.note("Stopped the game", false);
        }
        Effect::Login => {
            if let Some(task) = login_task.take() {
                task.abort();
            }
            match start_login(tx.clone(), app.client_id_input.clone()) {
                Ok(task) => *login_task = Some(task),
                Err(err) => app.note(err, true),
            }
        }
        Effect::CancelLogin => {
            if let Some(task) = login_task.take() {
                task.abort();
            }
            app.note("Sign-in cancelled", false);
        }
    }
}

fn persist(app: &App, config: &mut Config) -> weathered_core::Result<()> {
    config.game_dir = app.game_dir.display().to_string();
    config.memory = app.memory.clone();
    config.memory_min = app.memory_min.clone();
    config.jvm_args = app.jvm_args.clone();
    config.default_instance = app.default_instance.clone();
    config.instance_sort = app.sort_mode.label().to_string();
    config.download_sort = app.download_sort.label().to_string();
    config.java_path = app.java_path.clone();
    config.offline_username = app.username.clone();
    config.ms_client_id = app.client_id_input.clone();
    config.save()
}

struct LaunchParams {
    instance_id: String,
    game_dir: PathBuf,
    memory: String,
    username: String,
    java_path: String,
    offline: bool,
    account_uuid: String,
}

fn spawn_catalog(tx: mpsc::Sender<Msg>, game_dir: PathBuf) {
    tokio::spawn(async move {
        let client = match build_http_client() {
            Ok(client) => client,
            Err(err) => {
                let _ = tx
                    .send(Msg::Notice {
                        text: err.to_string(),
                        error: true,
                    })
                    .await;
                return;
            }
        };
        let paths = GamePaths::new(game_dir);
        match manifest::fetch_manifest(&client, &paths.manifest_cache()).await {
            Ok(fetched) => {
                let entries = fetched
                    .manifest
                    .versions
                    .into_iter()
                    .map(|entry| model::CatalogEntry {
                        id: entry.id,
                        kind: entry.version_type,
                        released: entry
                            .release_time
                            .as_deref()
                            .and_then(|time| time.split('T').next())
                            .unwrap_or("-")
                            .to_string(),
                    })
                    .collect();
                let _ = tx.send(Msg::Catalog(entries)).await;
            }
            Err(err) => {
                let _ = tx
                    .send(Msg::Notice {
                        text: err.to_string(),
                        error: true,
                    })
                    .await;
            }
        }
    });
}

fn spawn_javas(tx: mpsc::Sender<Msg>, game_dir: PathBuf) {
    tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for java in java::detect_system_javas() {
            let path = java.path.display().to_string();
            if seen.insert(path.clone()) {
                found.push(DetectedJava {
                    label: format!("{:<10} {path}", java.version),
                    path,
                });
            }
        }
        for java in java::detect_vendored_runtimes(&GamePaths::new(game_dir)) {
            let path = java.path.display().to_string();
            if seen.insert(path.clone()) {
                found.push(DetectedJava {
                    label: format!("{:<10} {path}  ({})", java.version, java.source),
                    path,
                });
            }
        }
        let _ = tx.blocking_send(Msg::Javas(found));
    });
}

struct DownloadTask {
    id: u64,
    cancel: CancelFlag,
    handle: JoinHandle<()>,
}

fn stop_download(task: &mut Option<DownloadTask>) {
    if let Some(task) = task.take() {
        task.cancel.cancel();
        task.handle.abort();
    }
}

fn stop_one_download(tasks: &mut Vec<DownloadTask>, id: u64) {
    if let Some(index) = tasks.iter().position(|task| task.id == id) {
        let task = tasks.remove(index);
        task.cancel.cancel();
        task.handle.abort();
    }
}

fn stop_all_downloads(tasks: &mut Vec<DownloadTask>) {
    for task in tasks.drain(..) {
        task.cancel.cancel();
        task.handle.abort();
    }
}

fn spawn_loader_list(
    tx: mpsc::Sender<Msg>,
    version: String,
    loader: String,
    _create_instance: bool,
    cancel: CancelFlag,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let sink = ProgressSink::noop();
        let outcome = async {
            cancel.check()?;
            let client = build_http_client()?;
            let parsed = Loader::parse(&loader)?;
            content::list_loader_builds(&client, parsed, &version, &cancel, &sink).await
        }
        .await;
        let (builds, error) = match outcome {
            Ok(builds) => (
                builds
                    .into_iter()
                    .map(|build| model::LoaderBuildRow {
                        version: build.version,
                        stable: build.stable,
                        recommended: build.recommended,
                    })
                    .collect(),
                String::new(),
            ),
            Err(err) if err.is_cancelled() => return,
            Err(err) => (Vec::new(), err.to_string()),
        };
        let _ = tx.send(Msg::LoaderBuilds { builds, error }).await;
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_install(
    tx: mpsc::Sender<Msg>,
    game_dir: PathBuf,
    id: u64,
    version: String,
    loader: String,
    loader_version: Option<String>,
    create_instance: bool,
    jobs: usize,
    cancel: CancelFlag,
) -> JoinHandle<()> {
    let progress_tx = tx.clone();
    let version_for_progress = version.clone();
    tokio::spawn(async move {
        let sink = ProgressSink::new(move |event| {
            let _ = progress_tx.try_send(Msg::Progress {
                id,
                version: version_for_progress.clone(),
                stage: event.stage,
                completed: event.completed,
                total: event.total,
                detail: event.detail,
                launching: false,
            });
        });
        let outcome = async {
            let client = build_http_client()?;
            let layout = DataLayout::new(&game_dir);
            let parsed = Loader::parse(&loader)?;
            let os = OsInfo::current();
            let item = content::download(
                &client,
                &layout,
                content::DownloadRequest {
                    version: &version,
                    loader: parsed,
                    loader_version: loader_version.as_deref(),
                    os: &os,
                    concurrency: jobs,
                    java_mode: JavaMode::Auto,
                    cancel: cancel.clone(),
                },
                &sink,
            )
            .await?;
            cancel.check()?;
            let mut detail = format!("stored {}", item.id);
            if create_instance {
                let name = if item.kind == "vanilla" {
                    item.game_version.clone()
                } else {
                    format!("{} {}", item.kind, item.game_version)
                };
                let instance = instance::create(&layout, &name, &item.id, "", "")?;
                detail = format!("instance {}", instance.name);
            }
            Ok::<_, weathered_core::Error>(detail)
        }
        .await;
        let (ok, detail) = match outcome {
            Ok(detail) => (true, detail),
            Err(err) if err.is_cancelled() => return,
            Err(err) => (false, err.to_string()),
        };
        let _ = tx
            .send(Msg::InstallFinished {
                id,
                version,
                ok,
                detail,
            })
            .await;
    })
}

fn spawn_create(tx: mpsc::Sender<Msg>, game_dir: PathBuf, catalog_id: String) {
    tokio::task::spawn_blocking(move || {
        let layout = DataLayout::new(&game_dir);
        let item = match weathered_core::catalog::CatalogItem::load(&layout, &catalog_id) {
            Ok(item) => item,
            Err(err) => {
                let _ = tx.blocking_send(Msg::Notice {
                    text: err.to_string(),
                    error: true,
                });
                return;
            }
        };
        match instance::create(&layout, &item.name, &catalog_id, "", "") {
            Ok(instance) => {
                let _ = tx.blocking_send(Msg::InstallFinished {
                    id: 0,
                    version: instance.game_version,
                    ok: true,
                    detail: format!("instance {}", instance.name),
                });
            }
            Err(err) => {
                let _ = tx.blocking_send(Msg::Notice {
                    text: err.to_string(),
                    error: true,
                });
            }
        }
    });
}

fn spawn_import(
    tx: mpsc::Sender<Msg>,
    game_dir: PathBuf,
    id: u64,
    path: String,
    jobs: usize,
    cancel: CancelFlag,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let sink = ProgressSink::new({
            let tx = tx.clone();
            move |event| {
                let _ = tx.try_send(Msg::Progress {
                    id,
                    version: "modpack".into(),
                    stage: event.stage,
                    completed: event.completed,
                    total: event.total,
                    detail: event.detail,
                    launching: false,
                });
            }
        });
        let client = match build_http_client() {
            Ok(client) => client,
            Err(err) => {
                let _ = tx
                    .send(Msg::Notice {
                        text: err.to_string(),
                        error: true,
                    })
                    .await;
                return;
            }
        };
        let layout = DataLayout::new(&game_dir);
        match instance::import_mrpack(
            &client,
            &layout,
            PathBuf::from(&path).as_path(),
            None,
            &OsInfo::current(),
            jobs,
            &sink,
            &cancel,
        )
        .await
        {
            Ok(instance) => {
                let _ = tx
                    .send(Msg::InstallFinished {
                        id,
                        version: instance.game_version,
                        ok: true,
                        detail: format!("imported {}", instance.name),
                    })
                    .await;
            }
            Err(err) if err.is_cancelled() => {}
            Err(err) => {
                let _ = tx
                    .send(Msg::Notice {
                        text: err.to_string(),
                        error: true,
                    })
                    .await;
            }
        }
    })
}

fn spawn_launch(tx: mpsc::Sender<Msg>, params: LaunchParams, control: LaunchControl, id: u64) {
    let progress_tx = tx.clone();
    let version_for_progress = params.instance_id.clone();
    tokio::spawn(async move {
        let sink = ProgressSink::new(move |event| {
            let _ = progress_tx.try_send(Msg::Progress {
                id,
                version: version_for_progress.clone(),
                stage: event.stage,
                completed: event.completed,
                total: event.total,
                detail: event.detail,
                launching: true,
            });
        });
        let started = async {
            if control.is_cancelled() {
                return Err(weathered_core::Error::message("launch cancelled"));
            }
            let client = build_http_client()?;
            let layout = DataLayout::new(&params.game_dir);
            let mut config = Config::load()?;
            let client_id = config.ensure_client_id();
            let session = if params.offline {
                weathered_core::auth::offline_session(&params.username)
            } else {
                microsoft_session(&client, &params.account_uuid, &config).await?
            };
            let plan = instance::prepare_launch(
                &client,
                &layout,
                &params.instance_id,
                LaunchRequest {
                    version_id: params.instance_id.clone(),
                    session,
                    java: if params.java_path.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(&params.java_path))
                    },
                    memory: params.memory.clone(),
                    os: OsInfo::current(),
                    features: FeatureSet::default(),
                    client_id,
                    quick_play_path: String::new(),
                    quick_play_singleplayer: String::new(),
                    quick_play_multiplayer: String::new(),
                    quick_play_realms: String::new(),
                    width: None,
                    height: None,
                    java_mode: if params.java_path.is_empty() {
                        JavaMode::Auto
                    } else {
                        JavaMode::Skip
                    },
                    concurrency: 16,
                },
                &sink,
            )
            .await?;
            if let Some(warning) = &plan.java_warning {
                let _ = tx
                    .send(Msg::Notice {
                        text: warning.clone(),
                        error: false,
                    })
                    .await;
            }
            if control.is_cancelled() {
                return Err(weathered_core::Error::message("launch cancelled"));
            }
            let mut child = spawn_captured(&plan)?;
            let _ = instance::touch_played(&layout, &params.instance_id);
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            control.store(child);
            let launch_log = create_launch_log(&params.game_dir).ok();
            let log_path = launch_log
                .as_ref()
                .map(|(path, _)| path.clone())
                .unwrap_or_default();
            let log_file = launch_log.map(|(_, file)| Arc::new(Mutex::new(file)));
            let mut pumps = Vec::new();
            if let Some(stdout) = stdout {
                pumps.push(pump(stdout, log_file.clone()));
            }
            if let Some(stderr) = stderr {
                pumps.push(pump(stderr, log_file));
            }
            watch_exit(
                control.clone(),
                tx.clone(),
                pumps,
                log_path,
                plan.working_dir.clone(),
                SystemTime::now(),
            );
            Ok::<(), weathered_core::Error>(())
        }
        .await;
        match started {
            Ok(()) => {
                let _ = tx.send(Msg::GameStarted { id }).await;
            }
            Err(err) => {
                let _ = tx
                    .send(Msg::LaunchFailed {
                        id,
                        detail: err.to_string(),
                    })
                    .await;
            }
        }
    });
}

fn pump(
    reader: impl Read + Send + 'static,
    log: Option<Arc<Mutex<File>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut lines = BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            if let Some(log) = &log {
                append_log(log, &line);
            }
        }
    })
}

fn watch_exit(
    control: LaunchControl,
    tx: mpsc::Sender<Msg>,
    pumps: Vec<thread::JoinHandle<()>>,
    log_path: PathBuf,
    game_dir: PathBuf,
    started: SystemTime,
) {
    thread::spawn(move || {
        let code = loop {
            thread::sleep(Duration::from_millis(200));
            if let Some(code) = control.poll_exit() {
                break code;
            }
        };
        for pump in pumps {
            let _ = pump.join();
        }
        let crash_report = model::newest_crash_report(&game_dir, started)
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let _ = tx.blocking_send(Msg::GameExit {
            code,
            log_path: log_path.display().to_string(),
            crash_report,
        });
    });
}

fn create_launch_log(data_dir: &Path) -> io::Result<(PathBuf, File)> {
    let dir = data_dir.join("logs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("launch-{}.log", utc_stamp()));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((path, file))
}

fn append_log(log: &Mutex<File>, line: &str) {
    if let Ok(mut file) = log.lock() {
        let _ = writeln!(file, "{line}");
    }
}

fn utc_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    format_unix(secs)
}

fn format_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

#[derive(Clone)]
struct LaunchControl {
    inner: Arc<LaunchInner>,
}

struct LaunchInner {
    cancel: std::sync::atomic::AtomicBool,
    child: Mutex<Option<Child>>,
}

impl LaunchControl {
    fn new() -> Self {
        Self {
            inner: Arc::new(LaunchInner {
                cancel: std::sync::atomic::AtomicBool::new(false),
                child: Mutex::new(None),
            }),
        }
    }

    fn cancel(&self) {
        self.inner
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.kill();
    }

    fn is_cancelled(&self) -> bool {
        self.inner.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn store(&self, child: Child) {
        if let Ok(mut guard) = self.inner.child.lock() {
            *guard = Some(child);
        }
    }

    fn kill(&self) {
        if let Ok(mut guard) = self.inner.child.lock()
            && let Some(child) = guard.as_mut()
        {
            let _ = child.kill();
        }
    }

    fn poll_exit(&self) -> Option<i32> {
        let mut guard = self.inner.child.lock().ok()?;
        let child = guard.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => Some(status.code().unwrap_or(1)),
            Ok(None) => None,
            Err(_) => Some(1),
        }
    }
}

async fn microsoft_session(
    client: &reqwest::Client,
    uuid: &str,
    config: &Config,
) -> weathered_core::Result<weathered_core::auth::Session> {
    use weathered_core::Error;
    let mut store = AccountStore::load()?;
    let account = store
        .get(uuid)
        .cloned()
        .ok_or_else(|| Error::message("that Microsoft account is no longer saved"))?;
    if !account.is_expired() {
        return Ok(account.session());
    }
    let env = std::env::var("WEATHERED_MS_CLIENT_ID").ok();
    let Some(client_id) = resolve_ms_client_id(None, env.as_deref(), config) else {
        return Err(Error::MissingClientId);
    };
    let auth = MicrosoftAuth::new(client.clone(), client_id)?;
    let refreshed = auth.refresh(&account).await?;
    let session = refreshed.session();
    store.upsert(refreshed);
    store.save()?;
    Ok(session)
}

fn start_login(tx: mpsc::Sender<Msg>, client_id: String) -> Result<JoinHandle<()>, String> {
    let config = Config::load().map_err(|err| err.to_string())?;
    let env = std::env::var("WEATHERED_MS_CLIENT_ID").ok();
    let Some(client_id) = resolve_ms_client_id(Some(&client_id), env.as_deref(), &config) else {
        return Err(
            "Set a Microsoft client id in Settings, or export WEATHERED_MS_CLIENT_ID.".into(),
        );
    };
    let tx_for_task = tx;
    Ok(tokio::spawn(async move {
        let client = match build_http_client() {
            Ok(client) => client,
            Err(err) => {
                let _ = tx_for_task
                    .send(Msg::LoginFinished {
                        ok: false,
                        detail: err.to_string(),
                        name: String::new(),
                    })
                    .await;
                return;
            }
        };
        let auth = match MicrosoftAuth::new(client, client_id) {
            Ok(auth) => auth,
            Err(err) => {
                let _ = tx_for_task
                    .send(Msg::LoginFinished {
                        ok: false,
                        detail: err.to_string(),
                        name: String::new(),
                    })
                    .await;
                return;
            }
        };
        let prompt_tx = tx_for_task.clone();
        let result = auth
            .login(|code| {
                let _ = prompt_tx.try_send(Msg::LoginPrompt {
                    url: code.verification_uri.clone(),
                    code: code.user_code.clone(),
                    message: code
                        .message
                        .clone()
                        .unwrap_or_else(|| "Waiting for you to approve the sign-in.".into()),
                });
            })
            .await;
        match result {
            Ok(account) => {
                let name = account.username.clone();
                let uuid = account.uuid.clone();
                if let Ok(mut store) = AccountStore::load() {
                    store.upsert(account);
                    store.set_active(&uuid);
                    let _ = store.save();
                }
                let _ = tx_for_task
                    .send(Msg::LoginFinished {
                        ok: true,
                        detail: String::new(),
                        name,
                    })
                    .await;
            }
            Err(err) => {
                let _ = tx_for_task
                    .send(Msg::LoginFinished {
                        ok: false,
                        detail: err.to_string(),
                        name: String::new(),
                    })
                    .await;
            }
        }
    }))
}

fn map_key(key: crossterm::event::KeyEvent) -> Option<Key> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        return Some(Key::CtrlC);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F'))
    {
        return Some(Key::CtrlF);
    }
    Some(match key.code {
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => Key::BackTab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => return None,
    })
}

fn map_mouse(mouse: MouseEvent) -> Option<Mouse> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => Some(Mouse::Click {
            x: mouse.column,
            y: mouse.row,
        }),
        MouseEventKind::ScrollDown => Some(Mouse::Scroll {
            x: mouse.column,
            y: mouse.row,
            down: true,
        }),
        MouseEventKind::ScrollUp => Some(Mouse::Scroll {
            x: mouse.column,
            y: mouse.row,
            down: false,
        }),
        MouseEventKind::Moved => Some(Mouse::Move {
            x: mouse.column,
            y: mouse.row,
        }),
        _ => None,
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            crossterm::event::EnableMouseCapture,
            cursor::Hide,
            terminal::SetTitle("Weathered Launcher")
        )
        .context("failed to enter the terminal UI")?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))
            .context("failed to start the terminal UI")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            cursor::Show
        );
    }
}

impl App {
    fn sync_profiles_from_disk(&mut self) {
        let microsoft = AccountStore::load()
            .map(|store| {
                store
                    .accounts
                    .into_iter()
                    .map(|account| Profile {
                        name: account.username,
                        detail: "microsoft".into(),
                        offline: false,
                        account_uuid: account.uuid,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.replace_profiles(microsoft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_stamp_formats_the_epoch() {
        assert_eq!(format_unix(0), "19700101-000000");
        assert_eq!(format_unix(86_400 + 3661), "19700102-010101");
    }

    #[test]
    fn launch_log_is_written_under_the_data_logs_directory() {
        let root = std::env::temp_dir().join(format!(
            "weathered-log-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (path, file) = create_launch_log(&root).unwrap();
        assert!(path.starts_with(root.join("logs")));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("launch-")
        );
        append_log(&Mutex::new(file), "log4j example");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("log4j example"));
        let _ = std::fs::remove_dir_all(root);
    }
}
