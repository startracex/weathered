//! Terminal UI state, keyboard handling, and mouse hit testing.
//!
//! Drawing lives in [`super::view`]. Background work reports [`Msg`] values
//! so the screen can keep painting while a download or launch is in progress.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

/// Top-level pages, in navigation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Launch,
    Download,
    Settings,
    More,
    /// Full-screen instance editor. It is not one of the Q/E tabs.
    Edit,
    /// Loader builds for one Minecraft version. It is not one of the Q/E tabs.
    Loaders,
}

impl Page {
    pub fn index(self) -> usize {
        match self {
            Self::Launch | Self::Edit => 0,
            Self::Download | Self::Loaders => 1,
            Self::Settings => 2,
            Self::More => 3,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Launch,
            1 => Self::Download,
            2 => Self::Settings,
            _ => Self::More,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Launch => "Launch",
            Self::Download => "Download",
            Self::Settings => "Settings",
            Self::More => "More",
            Self::Edit => "Edit",
            Self::Loaders => "Loaders",
        }
    }
}

/// Download-page categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Release,
    Snapshot,
    Old,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Release => "Release",
            Self::Snapshot => "Snapshot",
            Self::Old => "Old",
        }
    }

    pub fn loader(self) -> &'static str {
        "vanilla"
    }

    pub fn matches(self, kind: &str) -> bool {
        match self {
            Self::Release => kind == "release",
            Self::Snapshot => kind == "snapshot",
            Self::Old => kind == "old_beta" || kind == "old_alpha",
        }
    }
}

/// Which control keyboard input goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Versions,
    Accounts,
    /// Path in the top bar. Enter opens it in the file manager.
    DataDir,
    /// Launch control on the selected instance row.
    RowLaunch,
    /// Edit control on the selected instance row.
    RowEdit,
    DownloadCats,
    DownloadList,
    /// Install or Create instance on the selected version row.
    DownloadAction,
    /// Loaders button on the selected version row.
    DownloadLoaders,
    /// Download or Create instance on the selected loader build.
    LoaderButton,
    Search,
    Settings(usize),
    More,
}

/// A key the UI understands. Crossterm events are translated at the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    CtrlR,
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Backspace,
    Home,
    End,
    CtrlC,
    CtrlF,
}

/// Pointer input already translated from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    Click { x: u16, y: u16 },
    Scroll { x: u16, y: u16, down: bool },
    Move { x: u16, y: u16 },
}

/// Something the screen cannot do by itself, such as starting a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Install {
        id: u64,
        version: String,
        loader: String,
        loader_version: Option<String>,
        create_instance: bool,
    },
    OpenPicker {
        version: String,
        loader: String,
        force: bool,
    },
    DeleteInstance,
    SetDefault,
    OpenFolder,
    OpenDataDir,
    PersistDraft,
    CreateNamed {
        catalog_id: String,
        name: String,
    },
    ImportPack {
        id: u64,
        path: String,
    },
    CancelJob {
        id: u64,
    },
    Launch,
    StopGame,
    Save,
    Login,
    CancelLogin,
    RefreshManifest,
    /// Reload the version manifest and, on the loaders page, the open loader list.
    RefreshCache {
        force: bool,
    },
    Quit,
}

/// Messages from background tasks.
#[derive(Debug, Clone)]
pub enum Msg {
    Catalog {
        entries: Vec<CatalogEntry>,
        saved_at: i64,
    },
    Notice {
        text: String,
        error: bool,
    },
    Javas(Vec<DetectedJava>),
    Progress {
        id: u64,
        version: String,
        stage: String,
        completed: u64,
        total: u64,
        detail: String,
        launching: bool,
    },
    InstallFinished {
        id: u64,
        version: String,
        ok: bool,
        detail: String,
    },
    GameExit {
        code: i32,
        log_path: String,
        crash_report: String,
    },
    LoginPrompt {
        url: String,
        code: String,
        message: String,
    },
    LoginFinished {
        ok: bool,
        detail: String,
        name: String,
    },
    LoaderBuilds {
        game: String,
        loader: String,
        builds: Vec<LoaderBuildRow>,
        /// Unix time of the list that was just applied. Zero means "do not cache".
        saved_at: i64,
        error: String,
    },
    /// A catalog refresh finished, including one that failed after showing a cache.
    CatalogSettled,
    /// The game process is on screen and the Launch button should become Stop.
    GameStarted {
        id: u64,
    },
    /// Preparing the launch command failed before the process existed.
    LaunchFailed {
        id: u64,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub kind: String,
    pub released: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub kind: String,
}

/// One row on the Launch page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub game_version: String,
    pub loader: String,
    pub legacy: bool,
}

/// Loader build shown in the version picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderBuildRow {
    pub version: String,
    pub stable: bool,
    pub recommended: bool,
}

/// Fabric, Quilt, Forge, or NeoForge version list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderPicker {
    pub game_version: String,
    pub loader: String,
    pub builds: Vec<LoaderBuildRow>,
    pub show_unstable: bool,
    pub selected: usize,
    pub loading: bool,
    pub error: String,
    pub create_instance: bool,
}

/// How the instance list is ordered under the pinned default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Created,
    Played,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Created,
            Self::Created => Self::Played,
            Self::Played => Self::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Created => "created",
            Self::Played => "played",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "created" => Self::Created,
            "played" => Self::Played,
            _ => Self::Name,
        }
    }
}

/// How the download version list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadSort {
    /// Release date, newest first.
    Newest,
    /// Release date, oldest first.
    Oldest,
    /// Version name, low to high.
    Name,
    /// Versions already in the store, then newest.
    Stored,
}

impl DownloadSort {
    pub fn next(self) -> Self {
        match self {
            Self::Newest => Self::Oldest,
            Self::Oldest => Self::Name,
            Self::Name => Self::Stored,
            Self::Stored => Self::Newest,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Name => "name",
            Self::Stored => "stored",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "oldest" => Self::Oldest,
            "name" => Self::Name,
            "stored" => Self::Stored,
            _ => Self::Newest,
        }
    }
}

/// Editable copy of one instance. Empty override fields inherit globals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceDraft {
    pub id: String,
    pub name: String,
    pub description: String,
    pub java: String,
    pub memory_min: String,
    pub memory_max: String,
    pub jvm_args: String,
}

/// One downloaded profile that can become an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub game_version: String,
    pub launchable: bool,
    pub loader_version: String,
}

/// New-instance wizard step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatorStep {
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub detail: String,
    pub offline: bool,
    pub account_uuid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedJava {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub version: String,
    pub stage: String,
    pub completed: u64,
    pub total: u64,
    pub detail: String,
    pub launching: bool,
    pub create_instance: bool,
}

impl Job {
    pub fn percent(&self) -> u16 {
        percent(self.completed, self.total)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    GameDir,
    Memory,
    Username,
    JavaPath,
    ClientId,
    Search,
    PackPath,
    Rename,
    Description,
    JavaOverride,
    MemoryMin,
    MemoryMax,
    JvmArgs,
    MemoryMinGlobal,
    JvmGlobal,
    InstanceName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub field: Field,
    pub buffer: String,
    pub cursor: usize,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    pub label: String,
    pub value: String,
    pub kind: SettingKind,
    pub selected_mark: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Edit(Field),
    UseJava(usize),
    SignIn,
    Save,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPrompt {
    pub url: String,
    pub code: String,
    pub message: String,
}

/// All UI state.
#[derive(Debug, Clone)]
pub struct App {
    pub page: Page,
    pub focus: Focus,
    pub game_dir: PathBuf,
    pub memory: String,
    pub username: String,
    pub java_path: String,
    pub client_id_input: String,
    pub detected: Vec<DetectedJava>,
    pub installed: Vec<Installed>,
    pub installed_ids: HashSet<String>,
    pub instances: Vec<InstanceRow>,
    pub catalog_items: Vec<CatalogRow>,
    pub creator: Option<CreatorStep>,
    pub creator_selected: usize,
    pub picker: Option<LoaderPicker>,
    /// Catalog id waiting for the instance-name prompt.
    pub pending_catalog: Option<String>,
    /// Loader lists remembered for this session, keyed by `loader\ngame`.
    pub loader_cache: std::collections::HashMap<String, (i64, Vec<LoaderBuildRow>)>,
    pub search_open: bool,
    pub sort_mode: SortMode,
    pub download_sort: DownloadSort,
    pub default_instance: String,
    pub memory_min: String,
    pub jvm_args: String,
    pub draft: Option<InstanceDraft>,
    pub startup_select: bool,
    /// Unix time of the version list currently on screen.
    pub catalog_saved_at: i64,
    /// A manifest refresh is already running.
    pub catalog_refreshing: bool,
    /// Ids whose late progress must be ignored.
    pub cancelled_jobs: HashSet<u64>,
    next_job_id: u64,
    pub version_selected: usize,
    pub version_scroll: usize,
    pub version_window: usize,
    pub profiles: Vec<Profile>,
    pub profile_selected: usize,
    pub catalog: Vec<CatalogEntry>,
    pub catalog_ready: bool,
    pub category: Category,
    pub query: String,
    pub download_selected: usize,
    pub download_scroll: usize,
    pub download_window: usize,
    pub setting_selected: usize,
    pub jobs: Vec<Job>,
    pub status: String,
    pub status_error: bool,
    pub editing: Option<Editor>,
    pub login: Option<LoginPrompt>,
    pub game_running: bool,
    /// The user asked to stop the process, so a later non-zero exit is not a crash.
    pub stop_requested: bool,
    pub screen: Screen,
    pub hover: Option<(u16, u16)>,
    pub quit: bool,
}

impl App {
    /// Empty launcher state pointed at `game_dir`.
    pub fn empty(game_dir: PathBuf) -> Self {
        let mut app = Self {
            page: Page::Launch,
            focus: Focus::RowLaunch,
            game_dir,
            memory: "2G".into(),
            username: "Steve".into(),
            java_path: String::new(),
            client_id_input: String::new(),
            detected: Vec::new(),
            installed: Vec::new(),
            installed_ids: HashSet::new(),
            instances: Vec::new(),
            catalog_items: Vec::new(),
            creator: None,
            creator_selected: 0,
            picker: None,
            pending_catalog: None,
            loader_cache: std::collections::HashMap::new(),
            search_open: false,
            sort_mode: SortMode::Name,
            download_sort: DownloadSort::Newest,
            default_instance: String::new(),
            memory_min: String::new(),
            jvm_args: String::new(),
            draft: None,
            startup_select: true,
            catalog_saved_at: 0,
            catalog_refreshing: false,
            cancelled_jobs: HashSet::new(),
            next_job_id: 1,
            version_selected: 0,
            version_scroll: 0,
            version_window: 8,
            profiles: Vec::new(),
            profile_selected: 0,
            catalog: Vec::new(),
            catalog_ready: false,
            category: Category::Release,
            query: String::new(),
            download_selected: 0,
            download_scroll: 0,
            download_window: 12,
            setting_selected: 0,
            jobs: Vec::new(),
            status: "Ready".into(),
            status_error: false,
            editing: None,
            login: None,
            game_running: false,
            stop_requested: false,
            screen: Screen::default(),
            hover: None,
            quit: false,
        };
        app.sync_offline_profile();
        app
    }

    /// Populated state used by render tests and the text capture.
    #[cfg(test)]
    pub fn preview() -> Self {
        let mut app = Self::empty(PathBuf::from("/tmp/weathered-game"));
        app.installed = vec![
            Installed {
                id: "1.21.1".into(),
                kind: "release".into(),
            },
            Installed {
                id: "b1.7.3".into(),
                kind: "old_beta".into(),
            },
        ];
        app.rebuild_installed_ids();
        app.instances = vec![
            InstanceRow {
                id: "vanilla".into(),
                name: "Vanilla".into(),
                description: String::new(),
                game_version: "1.21.1".into(),
                loader: "vanilla".into(),
                legacy: false,
            },
            InstanceRow {
                id: "beta".into(),
                name: "Beta".into(),
                description: "old world".into(),
                game_version: "b1.7.3".into(),
                loader: "vanilla".into(),
                legacy: false,
            },
        ];
        app.catalog_items = vec![CatalogRow {
            id: "vanilla-1.21.1".into(),
            name: "1.21.1".into(),
            kind: "vanilla".into(),
            game_version: "1.21.1".into(),
            launchable: true,
            loader_version: String::new(),
        }];
        app.catalog_ready = true;
        app.catalog = vec![
            CatalogEntry {
                id: "1.21.1".into(),
                kind: "release".into(),
                released: "2024-08-08".into(),
            },
            CatalogEntry {
                id: "1.20.4".into(),
                kind: "release".into(),
                released: "2023-12-07".into(),
            },
            CatalogEntry {
                id: "24w14a".into(),
                kind: "snapshot".into(),
                released: "2024-04-03".into(),
            },
            CatalogEntry {
                id: "b1.7.3".into(),
                kind: "old_beta".into(),
                released: "2011-07-08".into(),
            },
        ];
        app.detected = vec![DetectedJava {
            label: "21.0.13   /usr/bin/java".into(),
            path: "/usr/bin/java".into(),
        }];
        app.default_instance = "vanilla".into();
        app.note("Ready", false);
        app
    }

    pub fn note(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_error = error;
    }

    pub fn filtered(&self) -> Vec<&CatalogEntry> {
        let query = self.query.to_ascii_lowercase();
        let mut items: Vec<&CatalogEntry> = self
            .catalog
            .iter()
            .filter(|entry| self.category.matches(&entry.kind))
            .filter(|entry| query.is_empty() || entry.id.to_ascii_lowercase().contains(&query))
            .collect();
        items.sort_by(|left, right| self.compare_download(left, right));
        items
    }

    fn compare_download(&self, left: &CatalogEntry, right: &CatalogEntry) -> std::cmp::Ordering {
        let by_date = right
            .released
            .cmp(&left.released)
            .then_with(|| version_cmp(&left.id, &right.id));
        match self.download_sort {
            DownloadSort::Newest => by_date,
            DownloadSort::Oldest => left
                .released
                .cmp(&right.released)
                .then_with(|| version_cmp(&left.id, &right.id)),
            DownloadSort::Name => version_cmp(&left.id, &right.id),
            DownloadSort::Stored => self
                .in_store(&right.id)
                .cmp(&self.in_store(&left.id))
                .then(by_date),
        }
    }

    fn cycle_download_sort(&mut self) {
        let keep = self
            .filtered()
            .get(self.download_selected)
            .map(|entry| entry.id.clone());
        self.download_sort = self.download_sort.next();
        if let Some(id) = keep
            && let Some(index) = self.filtered().iter().position(|entry| entry.id == id)
        {
            self.download_selected = index;
        }
        self.clamp_download();
    }

    pub fn setting_rows(&self) -> Vec<SettingRow> {
        let mut rows = vec![
            SettingRow {
                label: "Game directory".into(),
                value: self.game_dir.display().to_string(),
                kind: SettingKind::Edit(Field::GameDir),
                selected_mark: false,
            },
            SettingRow {
                label: "Memory max".into(),
                value: self.memory.clone(),
                kind: SettingKind::Edit(Field::Memory),
                selected_mark: false,
            },
            SettingRow {
                label: "Memory min".into(),
                value: if self.memory_min.is_empty() {
                    "unset".into()
                } else {
                    self.memory_min.clone()
                },
                kind: SettingKind::Edit(Field::MemoryMinGlobal),
                selected_mark: false,
            },
            SettingRow {
                label: "JVM args".into(),
                value: if self.jvm_args.is_empty() {
                    "unset".into()
                } else {
                    self.jvm_args.clone()
                },
                kind: SettingKind::Edit(Field::JvmGlobal),
                selected_mark: false,
            },
            SettingRow {
                label: "Username".into(),
                value: self.username.clone(),
                kind: SettingKind::Edit(Field::Username),
                selected_mark: false,
            },
            SettingRow {
                label: "Java path".into(),
                value: if self.java_path.is_empty() {
                    "auto".into()
                } else {
                    self.java_path.clone()
                },
                kind: SettingKind::Edit(Field::JavaPath),
                selected_mark: self.java_path.is_empty(),
            },
        ];
        for (index, java) in self.detected.iter().enumerate() {
            rows.push(SettingRow {
                label: "Use Java".into(),
                value: java.label.clone(),
                kind: SettingKind::UseJava(index),
                selected_mark: self.java_path == java.path,
            });
        }
        rows.push(SettingRow {
            label: "Microsoft client id".into(),
            value: if self.client_id_input.is_empty() {
                "not set".into()
            } else {
                self.client_id_input.clone()
            },
            kind: SettingKind::Edit(Field::ClientId),
            selected_mark: false,
        });
        rows.push(SettingRow {
            label: "Account".into(),
            value: "Sign in with Microsoft".into(),
            kind: SettingKind::SignIn,
            selected_mark: false,
        });
        rows.push(SettingRow {
            label: "Settings".into(),
            value: "Save".into(),
            kind: SettingKind::Save,
            selected_mark: false,
        });
        rows
    }

    pub fn reload_installed(&mut self) {
        self.installed = scan_installed(&self.game_dir);
        self.rebuild_installed_ids();
        self.sort_installed();
    }

    pub fn selected_instance(&self) -> Option<&InstanceRow> {
        self.instances.get(self.version_selected)
    }

    pub fn in_store(&self, version_id: &str) -> bool {
        let kind = self.category.loader();
        self.catalog_items
            .iter()
            .any(|item| item.kind == kind && item.game_version == version_id)
    }

    /// Reload instances and the download catalog from the data directory.
    pub fn reload_local(&mut self) {
        let layout = weathered_core::paths::DataLayout::new(&self.game_dir);
        let _ = weathered_core::instance::migrate_legacy(&layout);
        let mut found = weathered_core::instance::list(&layout).unwrap_or_default();
        let order = match self.sort_mode {
            SortMode::Name => weathered_core::instance::InstanceOrder::Name,
            SortMode::Created => weathered_core::instance::InstanceOrder::Created,
            SortMode::Played => weathered_core::instance::InstanceOrder::Played,
        };
        weathered_core::instance::order_instances(&mut found, order, &self.default_instance);
        let startup = self.startup_select;
        let keep = if self.startup_select {
            self.version_selected = 0;
            self.version_scroll = 0;
            self.startup_select = false;
            None
        } else {
            self.instances
                .get(self.version_selected)
                .map(|item| item.id.clone())
        };
        self.instances = found
            .into_iter()
            .map(|item| InstanceRow {
                id: item.id,
                name: item.name,
                description: item.description,
                game_version: item.game_version,
                loader: item.loader,
                legacy: item.legacy,
            })
            .collect();
        if let Some(id) = keep
            && let Some(index) = self.instances.iter().position(|item| item.id == id)
        {
            self.version_selected = index;
        }
        self.catalog_items = weathered_core::catalog::list(&layout)
            .unwrap_or_default()
            .into_iter()
            .map(|item| CatalogRow {
                id: item.id,
                name: item.name,
                kind: item.kind,
                game_version: item.game_version,
                launchable: item.launchable,
                loader_version: item.loader_version,
            })
            .collect();
        if !self.instances.is_empty() && self.version_selected >= self.instances.len() {
            self.version_selected = 0;
        }
        if startup {
            self.apply_startup_focus();
        }
    }

    /// The pinned default instance is the current selection.
    #[cfg(test)]
    pub fn selected_is_default(&self) -> bool {
        self.selected_instance().is_some_and(|item| {
            !self.default_instance.is_empty() && item.id == self.default_instance
        })
    }

    fn apply_startup_focus(&mut self) {
        self.version_selected = 0;
        self.version_scroll = 0;
        self.focus = if self.instances.is_empty() {
            Focus::Accounts
        } else {
            Focus::RowLaunch
        };
    }

    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profiles.get(self.profile_selected)
    }

    pub fn on_key(&mut self, key: Key) -> Effect {
        if key == Key::CtrlC {
            self.quit = true;
            return Effect::Quit;
        }
        if self.login.is_some() {
            if key == Key::Esc {
                self.login = None;
                return Effect::CancelLogin;
            }
            return Effect::None;
        }
        if self.editing.is_some() {
            return self.on_edit_key(key);
        }
        let key = match key {
            Key::Char(ch) => Key::Char(ch.to_ascii_lowercase()),
            other => other,
        };
        if self.search_open && key == Key::Esc {
            self.clear_search();
            return Effect::None;
        }
        if key == Key::Esc && self.page == Page::Loaders {
            self.page = Page::Download;
            self.focus = Focus::DownloadAction;
            return Effect::None;
        }
        if key == Key::Esc && !self.jobs.is_empty() {
            return self.cancel_job();
        }
        if key == Key::Esc && self.creator.is_some() {
            self.creator = None;
            return Effect::None;
        }
        if key == Key::Esc && self.page == Page::Edit {
            self.page = Page::Launch;
            self.focus = Focus::RowLaunch;
            return Effect::None;
        }
        if key == Key::CtrlF && self.page == Page::Download {
            self.search_open = true;
            self.begin_edit(Field::Search, self.query.clone());
            return Effect::None;
        }
        if key == Key::Char('o') && self.creator.is_none() {
            match self.page {
                Page::Launch => {
                    self.sort_mode = self.sort_mode.next();
                    self.reload_local();
                    self.note(format!("Sort by {}", self.sort_mode.label()), false);
                    return Effect::Save;
                }
                Page::Download => {
                    self.cycle_download_sort();
                    self.note(format!("Sort by {}", self.download_sort.label()), false);
                    return Effect::Save;
                }
                _ => {}
            }
        }
        if matches!(key, Key::Char('r') | Key::CtrlR)
            && matches!(self.page, Page::Download | Page::Loaders)
        {
            return Effect::RefreshCache { force: true };
        }
        if key == Key::Char('b') && self.page == Page::Loaders {
            if let Some(picker) = self.picker.as_mut() {
                picker.show_unstable = !picker.show_unstable;
            }
            self.align_picker_selection();
            return Effect::None;
        }
        if key == Key::Char('n') && self.page == Page::Launch && self.creator.is_none() {
            self.open_creator();
            return Effect::None;
        }
        match key {
            Key::Char('q') => {
                self.shift_page(-1);
                Effect::None
            }
            Key::Char('e') => {
                self.shift_page(1);
                Effect::None
            }
            Key::Char('a') => self.nudge_side(-1),
            Key::Char('d') => self.nudge_side(1),
            Key::Char('w') => {
                self.nudge(-1);
                Effect::None
            }
            Key::Char('s') => {
                self.nudge(1);
                Effect::None
            }
            Key::Char('1') => self.goto(Page::Launch),
            Key::Char('2') => self.goto(Page::Download),
            Key::Char('3') => self.goto(Page::Settings),
            Key::Char('4') => self.goto(Page::More),
            Key::Char('/') if self.page == Page::Download => {
                self.search_open = true;
                self.begin_edit(Field::Search, self.query.clone());
                Effect::None
            }
            Key::Tab => {
                self.cycle_focus(false);
                Effect::None
            }
            Key::BackTab => {
                self.cycle_focus(true);
                Effect::None
            }
            Key::Up | Key::Char('k') => {
                self.nudge(-1);
                Effect::None
            }
            Key::Down | Key::Char('j') => {
                self.nudge(1);
                Effect::None
            }
            Key::Left => self.nudge_side(-1),
            Key::Right => self.nudge_side(1),
            Key::Enter => self.activate(),
            _ => Effect::None,
        }
    }

    pub fn on_mouse(&mut self, mouse: Mouse) -> Effect {
        match mouse {
            Mouse::Move { x, y } => {
                self.hover = Some((x, y));
                Effect::None
            }
            Mouse::Scroll { x, y, down } => {
                self.scroll_at(x, y, if down { 3 } else { -3 });
                Effect::None
            }
            Mouse::Click { x, y } => self.click_at(x, y),
        }
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::LoaderBuilds {
                game,
                loader,
                builds,
                saved_at,
                error,
            } => {
                if !builds.is_empty() && saved_at > 0 {
                    self.loader_cache.insert(
                        Self::loader_cache_key(&loader, &game),
                        (saved_at, builds.clone()),
                    );
                }
                let current = self
                    .picker
                    .as_ref()
                    .is_some_and(|picker| picker.game_version == game && picker.loader == loader);
                if !current {
                    return;
                }
                let keep_existing = builds.is_empty()
                    && self
                        .picker
                        .as_ref()
                        .is_some_and(|picker| !picker.builds.is_empty());
                if let Some(picker) = self.picker.as_mut() {
                    picker.loading = false;
                    if !keep_existing {
                        picker.error = error.clone();
                        picker.builds = builds;
                        picker.selected = 0;
                    }
                }
                if keep_existing && !error.is_empty() {
                    self.note(error, true);
                } else if !keep_existing {
                    self.align_picker_selection();
                }
            }
            Msg::Catalog { entries, saved_at } => {
                self.catalog = entries;
                self.catalog_ready = true;
                self.catalog_saved_at = saved_at;
                self.clamp_download();
                self.sort_installed();
                if self.status == "Loading version list..."
                    || self.status == "Refreshing version list..."
                    || self.status == "Ready"
                {
                    self.note(format!("{} versions", self.catalog.len()), false);
                }
            }
            Msg::CatalogSettled => {
                self.catalog_refreshing = false;
            }
            Msg::Notice { text, error } => self.note(text, error),
            Msg::Javas(found) => {
                self.detected = found;
                if self.setting_selected >= self.setting_rows().len() {
                    self.setting_selected = 0;
                }
            }
            Msg::Progress {
                id,
                version,
                stage,
                completed,
                total,
                detail,
                launching,
            } => {
                if self.cancelled_jobs.contains(&id) {
                    return;
                }
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    job.version = version;
                    job.stage = stage;
                    job.completed = completed;
                    job.total = total;
                    job.detail = detail;
                    job.launching = launching;
                }
            }
            Msg::InstallFinished {
                id,
                version,
                ok,
                detail,
            } => {
                self.jobs.retain(|job| job.id != id);
                if self.cancelled_jobs.contains(&id) {
                    return;
                }
                if ok {
                    self.reload_installed();
                    self.reload_local();
                    self.note(format!("Stored {version}. {detail}"), false);
                    if self.jobs.iter().all(|job| job.launching) && detail.contains("instance") {
                        self.page = Page::Launch;
                        self.focus = Focus::RowLaunch;
                    }
                } else {
                    self.note(format!("Install {version} failed: {detail}"), true);
                }
            }
            Msg::GameExit {
                code,
                log_path,
                crash_report,
            } => {
                self.game_running = false;
                self.jobs.retain(|job| !job.launching);
                if self.stop_requested {
                    self.stop_requested = false;
                    self.note("Stopped", false);
                } else {
                    let (text, error) = exit_notice(code, &log_path, &crash_report);
                    self.note(text, error);
                }
            }
            Msg::LoginPrompt { url, code, message } => {
                self.login = Some(LoginPrompt { url, code, message });
            }
            Msg::LoginFinished { ok, detail, name } => {
                self.login = None;
                if ok {
                    self.note(format!("Signed in as {name}"), false);
                } else {
                    self.note(detail, true);
                }
            }
            Msg::GameStarted { id } => {
                if self.cancelled_jobs.contains(&id) {
                    return;
                }
                self.jobs.retain(|job| job.id != id);
                self.game_running = true;
                self.note("Game is running", false);
            }
            Msg::LaunchFailed { id, detail } => {
                self.jobs.retain(|job| job.id != id);
                if self.cancelled_jobs.contains(&id) {
                    return;
                }
                self.game_running = false;
                self.note(detail, true);
            }
        }
    }

    fn shift_page(&mut self, delta: isize) {
        let next = (self.page.index() as isize + delta).rem_euclid(4) as usize;
        let _ = self.goto(Page::from_index(next));
    }

    pub fn loader_scroll(&self, window: usize) -> usize {
        let selected = self
            .picker
            .as_ref()
            .map(|picker| picker.selected)
            .unwrap_or(0);
        if window == 0 || selected < window {
            0
        } else {
            selected + 1 - window
        }
    }

    pub fn picker_visible(&self) -> Vec<LoaderBuildRow> {
        let Some(picker) = &self.picker else {
            return Vec::new();
        };
        picker
            .builds
            .iter()
            .filter(|build| picker.show_unstable || build.stable)
            .cloned()
            .collect()
    }

    fn align_picker_selection(&mut self) {
        let visible = self.picker_visible();
        let builds: Vec<weathered_core::catalog::LoaderBuild> = visible
            .iter()
            .map(|build| weathered_core::catalog::LoaderBuild {
                version: build.version.clone(),
                stable: build.stable,
                recommended: build.recommended,
            })
            .collect();
        if let Some(picker) = self.picker.as_mut() {
            picker.selected = weathered_core::catalog::preferred_index(&builds)
                .min(visible.len().saturating_sub(1));
        }
    }

    fn goto(&mut self, page: Page) -> Effect {
        self.page = page;
        self.focus = match page {
            Page::Launch => {
                if self.instances.is_empty() {
                    Focus::Accounts
                } else {
                    Focus::RowLaunch
                }
            }
            Page::Download => Focus::DownloadAction,
            Page::Loaders => Focus::LoaderButton,
            Page::Settings | Page::Edit => Focus::Settings(self.setting_selected),
            Page::More => Focus::More,
        };
        if page == Page::Download && self.needs_catalog_refresh() {
            Effect::RefreshCache { force: false }
        } else {
            Effect::None
        }
    }

    fn needs_catalog_refresh(&self) -> bool {
        !self.catalog_refreshing
            && !weathered_core::cache::is_fresh(
                self.catalog_saved_at,
                weathered_core::cache::FRESH_SECS,
            )
    }

    /// Show a cached Mojang manifest immediately. A stale file stays on screen
    /// until the background refresh replaces it.
    pub fn load_cached_catalog(&mut self) {
        let path = weathered_core::paths::DataLayout::new(&self.game_dir)
            .cache_dir()
            .join("version_manifest_v2.json");
        let Some((saved, bytes)) = weathered_core::cache::read(&path) else {
            return;
        };
        let Ok(manifest) = weathered_core::manifest::parse_manifest(&bytes) else {
            return;
        };
        self.catalog = entries_from_manifest(&manifest);
        self.catalog_ready = !self.catalog.is_empty();
        self.catalog_saved_at = saved;
    }

    fn cycle_focus(&mut self, reverse: bool) {
        self.focus = match (self.page, self.focus, reverse) {
            (Page::Launch, Focus::RowLaunch, false) => Focus::RowEdit,
            (Page::Launch, Focus::RowEdit, false) => Focus::Accounts,
            (Page::Launch, Focus::Accounts, false) => Focus::DataDir,
            (Page::Launch, _, false) => Focus::RowLaunch,
            (Page::Launch, Focus::RowLaunch, true) => Focus::DataDir,
            (Page::Launch, Focus::DataDir, true) => Focus::Accounts,
            (Page::Launch, Focus::Accounts, true) => Focus::RowEdit,
            (Page::Launch, _, true) => Focus::RowLaunch,
            (Page::Download, Focus::DownloadList, false) => Focus::DownloadCats,
            (Page::Download, Focus::DownloadCats, false) if self.search_open => Focus::Search,
            (Page::Download, _, false) => Focus::DownloadList,
            (Page::Download, Focus::DownloadList, true) if self.search_open => Focus::Search,
            (Page::Download, Focus::Search, true) => Focus::DownloadCats,
            (Page::Download, Focus::DownloadList, true) => Focus::DownloadCats,
            (Page::Download, _, true) => Focus::DownloadList,
            (Page::Settings | Page::Edit, _, _) => Focus::Settings(self.setting_selected),
            (Page::More, _, _) => Focus::More,
            (Page::Loaders, _, _) => Focus::LoaderButton,
        };
    }

    fn nudge(&mut self, delta: isize) {
        match self.page {
            Page::Launch if self.focus == Focus::Accounts => {
                bump(
                    &mut self.profile_selected,
                    self.profiles.len(),
                    delta.signum(),
                );
            }
            Page::Launch if self.focus == Focus::DataDir => {}
            Page::Launch if self.creator.is_some() => {
                let len = self.creator_len();
                bump(&mut self.creator_selected, len, delta.signum());
            }
            Page::Launch => {
                let column = if self.focus == Focus::RowEdit {
                    Focus::RowEdit
                } else {
                    Focus::RowLaunch
                };
                slide(
                    &mut self.version_selected,
                    &mut self.version_scroll,
                    self.instances.len(),
                    self.version_window,
                    delta,
                );
                self.focus = column;
            }
            Page::Download if self.focus == Focus::DownloadCats => {
                self.shift_category(delta.signum());
            }
            Page::Download => {
                let column = if self.focus == Focus::DownloadLoaders {
                    Focus::DownloadLoaders
                } else {
                    Focus::DownloadAction
                };
                self.focus = column;
                let len = self.filtered().len();
                slide(
                    &mut self.download_selected,
                    &mut self.download_scroll,
                    len,
                    self.download_window,
                    delta,
                );
            }
            Page::Settings => {
                let len = self.setting_rows().len();
                bump(&mut self.setting_selected, len, delta.signum());
                self.focus = Focus::Settings(self.setting_selected);
            }
            Page::More => {}
            Page::Edit => {
                bump(&mut self.setting_selected, 9, delta.signum());
                self.focus = Focus::Settings(self.setting_selected);
            }
            Page::Loaders => {
                let len = self.picker_visible().len();
                if let Some(picker) = self.picker.as_mut() {
                    bump(&mut picker.selected, len, delta.signum());
                }
                self.focus = Focus::LoaderButton;
            }
        }
    }

    fn nudge_side(&mut self, delta: isize) -> Effect {
        if self.page == Page::Edit {
            return Effect::None;
        }
        if self.page == Page::Launch && self.creator.is_none() {
            self.focus = if delta > 0 {
                Focus::RowEdit
            } else {
                Focus::RowLaunch
            };
            return Effect::None;
        }
        if self.page == Page::Download
            && matches!(
                self.focus,
                Focus::DownloadAction | Focus::DownloadLoaders | Focus::DownloadList
            )
        {
            self.focus = if delta > 0 {
                Focus::DownloadLoaders
            } else {
                Focus::DownloadAction
            };
            return Effect::None;
        }
        if self.page == Page::Loaders {
            return self.shift_loader(delta);
        }
        if self.focus == Focus::DownloadCats {
            self.shift_category(delta.signum());
            return Effect::None;
        }
        let next = self.page.index() as isize + delta.signum();
        let next = next.clamp(0, 3) as usize;
        self.goto(Page::from_index(next))
    }

    fn shift_category(&mut self, delta: isize) {
        let order = [Category::Release, Category::Snapshot, Category::Old];
        let index = order
            .iter()
            .position(|item| *item == self.category)
            .unwrap_or(0) as isize;
        let next = (index + delta).clamp(0, order.len() as isize - 1) as usize;
        self.category = order[next];
        self.download_selected = 0;
        self.download_scroll = 0;
        self.focus = Focus::DownloadCats;
    }

    fn activate(&mut self) -> Effect {
        if self.creator.is_some() && self.page == Page::Launch {
            return self.activate_creator();
        }
        if self.focus == Focus::DataDir {
            return Effect::OpenDataDir;
        }
        match self.page {
            Page::Launch if self.focus == Focus::RowEdit => self.open_edit(),
            Page::Launch if self.focus == Focus::Accounts => Effect::None,
            Page::Launch => self.request_launch(),
            Page::Edit => self.activate_edit(),
            Page::Loaders => self.activate_loader(),
            Page::Download if self.focus == Focus::DownloadLoaders => self.open_loaders(),
            Page::Download if self.focus == Focus::Search => {
                self.begin_edit(Field::Search, self.query.clone());
                Effect::None
            }
            Page::Download if self.focus == Focus::DownloadCats => Effect::None,
            Page::Download => self.activate_download_action(),
            Page::Settings => self.activate_setting(self.setting_selected),
            Page::More => Effect::RefreshManifest,
        }
    }

    fn click_at(&mut self, x: u16, y: u16) -> Effect {
        let Some(action) = self
            .screen
            .zones
            .iter()
            .rev()
            .find(|zone| contains(zone.rect, x, y))
            .map(|zone| zone.action)
        else {
            return Effect::None;
        };
        if self.login.is_some() && !matches!(action, Click::CancelLogin) {
            return Effect::None;
        }
        if self.editing.is_some() {
            let effect = self.commit_edit();
            if self.editing.is_some() || effect != Effect::None {
                return effect;
            }
        }
        match action {
            Click::Page(page) => self.goto(page),
            Click::Account(index) => {
                self.page = Page::Launch;
                self.focus = Focus::Accounts;
                self.profile_selected = index;
                Effect::None
            }
            Click::CancelJob => self.cancel_job(),
            Click::NewInstance => {
                self.open_creator();
                Effect::None
            }
            Click::Creator(index) => {
                self.creator_selected = index;
                self.activate_creator()
            }
            Click::Version(index) => {
                self.page = Page::Launch;
                self.version_selected = index;
                self.focus = Focus::RowLaunch;
                Effect::None
            }
            Click::RowEdit(index) => {
                self.page = Page::Launch;
                self.version_selected = index;
                self.focus = Focus::RowEdit;
                self.open_edit()
            }
            Click::RowLaunch(index) => {
                self.page = Page::Launch;
                self.version_selected = index;
                self.focus = Focus::RowLaunch;
                self.request_launch()
            }
            Click::OpenDataDir => {
                self.focus = Focus::DataDir;
                Effect::OpenDataDir
            }
            Click::Category(category) => {
                self.page = Page::Download;
                self.focus = Focus::DownloadCats;
                self.category = category;
                self.download_selected = 0;
                self.download_scroll = 0;
                Effect::None
            }
            Click::DownloadRow(index) => {
                self.page = Page::Download;
                self.focus = Focus::DownloadAction;
                self.download_selected = index;
                Effect::None
            }
            Click::InstallRow(index) => {
                self.page = Page::Download;
                self.focus = Focus::DownloadAction;
                self.download_selected = index;
                self.activate_download_action()
            }
            Click::LoadersRow(index) => {
                self.page = Page::Download;
                self.focus = Focus::DownloadLoaders;
                self.download_selected = index;
                self.open_loaders()
            }
            Click::LoaderKind(index) => self.shift_loader_to(index),
            Click::LoaderSelect(index) => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.selected = index;
                }
                self.focus = Focus::LoaderButton;
                Effect::None
            }
            Click::LoaderAction(index) => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.selected = index;
                }
                self.focus = Focus::LoaderButton;
                self.activate_loader()
            }
            Click::Setting(index) => {
                if self.page == Page::Edit {
                    self.setting_selected = index;
                    self.focus = Focus::Settings(index);
                    return self.activate_edit();
                }
                self.page = Page::Settings;
                self.setting_selected = index;
                self.focus = Focus::Settings(index);
                self.activate_setting(index)
            }
            Click::Search => {
                self.page = Page::Download;
                self.focus = Focus::Search;
                self.begin_edit(Field::Search, self.query.clone());
                Effect::None
            }
            Click::SignIn => {
                self.focus = Focus::Settings(self.setting_selected);
                Effect::Login
            }
            Click::Save => Effect::Save,
            Click::Refresh => Effect::RefreshManifest,
            Click::CancelLogin => {
                self.login = None;
                Effect::CancelLogin
            }
        }
    }

    fn scroll_at(&mut self, x: u16, y: u16, delta: isize) {
        if contains(self.screen.version_list, x, y) {
            if self.focus != Focus::RowEdit {
                self.focus = Focus::RowLaunch;
            }
            slide(
                &mut self.version_selected,
                &mut self.version_scroll,
                self.instances.len(),
                self.version_window,
                delta,
            );
        } else if contains(self.screen.download_list, x, y) {
            if self.focus != Focus::DownloadLoaders {
                self.focus = Focus::DownloadAction;
            }
            let len = self.filtered().len();
            slide(
                &mut self.download_selected,
                &mut self.download_scroll,
                len,
                self.download_window,
                delta,
            );
        } else if contains(self.screen.account_list, x, y) {
            self.focus = Focus::Accounts;
            bump(
                &mut self.profile_selected,
                self.profiles.len(),
                delta.signum(),
            );
        } else if contains(self.screen.settings_list, x, y) {
            let len = self.setting_rows().len();
            bump(&mut self.setting_selected, len, delta.signum());
            self.focus = Focus::Settings(self.setting_selected);
        }
    }

    fn start_job(&mut self, version: String, launching: bool, create_instance: bool) -> u64 {
        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.saturating_add(1);
        self.jobs.push(Job {
            id,
            version,
            stage: if launching {
                "preparing".into()
            } else {
                "starting".into()
            },
            completed: 0,
            total: if launching { 0 } else { 1 },
            detail: if launching {
                "Resolving files".into()
            } else {
                "Queued".into()
            },
            launching,
            create_instance,
        });
        id
    }

    pub fn vanilla_item(&self, version_id: &str) -> Option<&CatalogRow> {
        self.catalog_items.iter().find(|item| {
            item.kind == "vanilla" && item.game_version == version_id && item.launchable
        })
    }

    pub fn loader_item(&self, kind: &str, game: &str, loader_version: &str) -> Option<&CatalogRow> {
        self.catalog_items.iter().find(|item| {
            item.kind == kind
                && item.game_version == game
                && item.loader_version == loader_version
                && item.launchable
        })
    }

    fn prompt_instance_name(&mut self, catalog_id: String, default_name: String) {
        self.pending_catalog = Some(catalog_id);
        self.begin_edit(Field::InstanceName, default_name);
    }

    fn activate_download_action(&mut self) -> Effect {
        let Some(version) = self
            .filtered()
            .get(self.download_selected)
            .map(|entry| entry.id.clone())
        else {
            return Effect::None;
        };
        if let Some(item) = self.vanilla_item(&version) {
            let catalog_id = item.id.clone();
            self.prompt_instance_name(catalog_id, version);
            Effect::None
        } else {
            self.request_install(self.download_selected)
        }
    }

    fn open_loaders(&mut self) -> Effect {
        let Some(version) = self
            .filtered()
            .get(self.download_selected)
            .map(|entry| entry.id.clone())
        else {
            return Effect::None;
        };
        self.page = Page::Loaders;
        self.focus = Focus::LoaderButton;
        self.picker = Some(LoaderPicker {
            game_version: version.clone(),
            loader: "fabric".into(),
            builds: Vec::new(),
            show_unstable: false,
            selected: 0,
            loading: true,
            error: String::new(),
            create_instance: false,
        });
        self.fill_loader_from_cache();
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| !picker.loading && picker.error.is_empty())
        {
            Effect::None
        } else {
            Effect::OpenPicker {
                version,
                loader: "fabric".into(),
                force: false,
            }
        }
    }

    fn loader_cache_key(loader: &str, game: &str) -> String {
        format!("{loader}\n{game}")
    }

    fn fill_loader_from_cache(&mut self) {
        let Some(picker) = self.picker.clone() else {
            return;
        };
        let key = Self::loader_cache_key(&picker.loader, &picker.game_version);
        if let Some((saved, builds)) = self.loader_cache.get(&key).cloned() {
            if let Some(picker) = self.picker.as_mut() {
                picker.builds = builds;
                picker.loading =
                    !weathered_core::cache::is_fresh(saved, weathered_core::cache::FRESH_SECS);
                picker.error.clear();
            }
            self.align_picker_selection();
            return;
        }
        let path = weathered_core::content::loader_cache_path(
            &weathered_core::paths::DataLayout::new(&self.game_dir),
            &picker.loader,
            &picker.game_version,
        );
        if let Some((saved, builds)) = weathered_core::content::read_loader_cache(&path) {
            let rows = builds
                .into_iter()
                .map(|build| LoaderBuildRow {
                    version: build.version,
                    stable: build.stable,
                    recommended: build.recommended,
                })
                .collect::<Vec<_>>();
            self.loader_cache.insert(key, (saved, rows.clone()));
            if let Some(picker) = self.picker.as_mut() {
                picker.builds = rows;
                picker.loading =
                    !weathered_core::cache::is_fresh(saved, weathered_core::cache::FRESH_SECS);
                picker.error.clear();
            }
            self.align_picker_selection();
        }
    }

    fn loader_index(&self) -> usize {
        let kinds = ["fabric", "quilt", "forge", "neoforge"];
        self.picker
            .as_ref()
            .and_then(|picker| kinds.iter().position(|kind| *kind == picker.loader))
            .unwrap_or(0)
    }

    fn shift_loader(&mut self, delta: isize) -> Effect {
        let next = (self.loader_index() as isize + delta.signum()).clamp(0, 3) as usize;
        self.shift_loader_to(next)
    }

    fn shift_loader_to(&mut self, next: usize) -> Effect {
        let kinds = ["fabric", "quilt", "forge", "neoforge"];
        let Some(picker) = self.picker.as_ref() else {
            return Effect::None;
        };
        let next = next.min(kinds.len() - 1);
        if picker.loader == kinds[next] {
            return Effect::None;
        }
        let loader = kinds[next].to_string();
        let game = picker.game_version.clone();
        if let Some(picker) = self.picker.as_mut() {
            picker.loader = loader.clone();
            picker.selected = 0;
            picker.builds.clear();
            picker.loading = true;
            picker.error.clear();
        }
        self.focus = Focus::LoaderButton;
        self.fill_loader_from_cache();
        if self.picker.as_ref().is_some_and(|picker| picker.loading) {
            Effect::OpenPicker {
                version: game,
                loader,
                force: false,
            }
        } else {
            Effect::None
        }
    }

    fn activate_loader(&mut self) -> Effect {
        let Some(picker) = self.picker.clone() else {
            return Effect::None;
        };
        let visible = self.picker_visible();
        let Some(build) = visible.get(picker.selected) else {
            return Effect::None;
        };
        if let Some(item) = self.loader_item(&picker.loader, &picker.game_version, &build.version) {
            let catalog_id = item.id.clone();
            let name = format!("{}-{}", picker.game_version, picker.loader);
            self.prompt_instance_name(catalog_id, name);
            return Effect::None;
        }
        let label = format!(
            "{} {} {}",
            picker.game_version, picker.loader, build.version
        );
        if self
            .jobs
            .iter()
            .any(|job| !job.launching && job.version == label)
        {
            self.note(format!("{label} is already downloading"), false);
            return Effect::None;
        }
        let id = self.start_job(label.clone(), false, false);
        self.note(format!("Downloading {label}"), false);
        Effect::Install {
            id,
            version: picker.game_version,
            loader: picker.loader,
            loader_version: Some(build.version.clone()),
            create_instance: false,
        }
    }

    fn request_launch(&mut self) -> Effect {
        if self.game_running {
            return Effect::StopGame;
        }
        if self.jobs.iter().any(|job| job.launching) {
            self.note("A launch is already being prepared", true);
            return Effect::None;
        }
        let Some(instance) = self.selected_instance() else {
            self.note("Create an instance first", true);
            return Effect::None;
        };
        if self.jobs.iter().any(|job| {
            job.create_instance
                && !job.launching
                && (job.version == instance.game_version || job.version == instance.id)
        }) {
            self.note("That instance is still being installed", true);
            return Effect::None;
        }
        let name = instance.name.clone();
        let _id = self.start_job(name.clone(), true, false);
        self.note(format!("Preparing {name}"), false);
        Effect::Launch
    }

    fn request_install(&mut self, index: usize) -> Effect {
        let Some(version) = self.filtered().get(index).map(|entry| entry.id.clone()) else {
            return Effect::None;
        };
        if self
            .jobs
            .iter()
            .any(|job| !job.launching && job.version == version)
        {
            self.note(format!("{version} is already downloading"), false);
            return Effect::None;
        }
        self.download_selected = index;
        let id = self.start_job(version.clone(), false, false);
        self.note(format!("Downloading {version}"), false);
        Effect::Install {
            id,
            version,
            loader: "vanilla".into(),
            loader_version: None,
            create_instance: false,
        }
    }

    fn open_edit(&mut self) -> Effect {
        let Some(row) = self.selected_instance() else {
            self.note("Select an instance first", true);
            return Effect::None;
        };
        let layout = weathered_core::paths::DataLayout::new(&self.game_dir);
        let Ok(Some(instance)) = weathered_core::instance::find(&layout, &row.id) else {
            self.note("That instance is no longer on disk", true);
            return Effect::None;
        };
        self.draft = Some(InstanceDraft {
            id: instance.id,
            name: instance.name,
            description: instance.description,
            java: instance.java,
            memory_min: instance.memory_min,
            memory_max: instance.memory,
            jvm_args: instance.jvm_args,
        });
        self.page = Page::Edit;
        self.setting_selected = 0;
        self.focus = Focus::Settings(0);
        Effect::None
    }

    fn activate_edit(&mut self) -> Effect {
        let Some(draft) = self.draft.clone() else {
            self.page = Page::Launch;
            return Effect::None;
        };
        match self.setting_selected {
            0 => {
                self.begin_edit(Field::Rename, draft.name);
                Effect::None
            }
            1 => {
                self.begin_edit(Field::Description, draft.description);
                Effect::None
            }
            2 => Effect::SetDefault,
            3 => {
                self.begin_edit(Field::JavaOverride, draft.java);
                Effect::None
            }
            4 => {
                self.begin_edit(Field::MemoryMin, draft.memory_min);
                Effect::None
            }
            5 => {
                self.begin_edit(Field::MemoryMax, draft.memory_max);
                Effect::None
            }
            6 => {
                self.begin_edit(Field::JvmArgs, draft.jvm_args);
                Effect::None
            }
            7 => Effect::OpenFolder,
            _ => {
                self.page = Page::Launch;
                Effect::DeleteInstance
            }
        }
    }

    fn clear_search(&mut self) {
        self.query.clear();
        self.search_open = false;
        self.editing = None;
        self.download_selected = 0;
        self.download_scroll = 0;
        self.focus = Focus::DownloadAction;
    }

    fn open_creator(&mut self) {
        self.creator = Some(CreatorStep::Source);
        self.creator_selected = 0;
        self.page = Page::Launch;
        self.focus = Focus::Versions;
        self.note("New instance", false);
    }

    fn creator_len(&self) -> usize {
        match self.creator {
            Some(CreatorStep::Source) => 2,
            None => 0,
        }
    }

    fn activate_creator(&mut self) -> Effect {
        match self.creator {
            Some(CreatorStep::Source) => match self.creator_selected {
                0 => {
                    self.creator = None;
                    self.page = Page::Download;
                    self.focus = Focus::DownloadAction;
                    self.note(
                        "Choose a version to install or turn into an instance.",
                        false,
                    );
                    if self.needs_catalog_refresh() {
                        Effect::RefreshCache { force: false }
                    } else {
                        Effect::None
                    }
                }
                _ => {
                    self.begin_edit(Field::PackPath, String::new());
                    Effect::None
                }
            },
            None => Effect::None,
        }
    }

    fn activate_setting(&mut self, index: usize) -> Effect {
        let Some(row) = self.setting_rows().get(index).cloned() else {
            return Effect::None;
        };
        match row.kind {
            SettingKind::Edit(field) => {
                let current = match field {
                    Field::GameDir => self.game_dir.display().to_string(),
                    Field::Memory => self.memory.clone(),
                    Field::Username => self.username.clone(),
                    Field::JavaPath => self.java_path.clone(),
                    Field::ClientId => self.client_id_input.clone(),
                    Field::Search | Field::PackPath => self.query.clone(),
                    Field::Rename => self
                        .selected_instance()
                        .map(|item| item.name.clone())
                        .unwrap_or_default(),
                    Field::Description => self
                        .selected_instance()
                        .map(|item| item.description.clone())
                        .unwrap_or_default(),
                    Field::JavaOverride => self
                        .draft
                        .as_ref()
                        .map(|draft| draft.java.clone())
                        .unwrap_or_default(),
                    Field::MemoryMin => self
                        .draft
                        .as_ref()
                        .map(|draft| draft.memory_min.clone())
                        .unwrap_or_default(),
                    Field::MemoryMax => self
                        .draft
                        .as_ref()
                        .map(|draft| draft.memory_max.clone())
                        .unwrap_or_default(),
                    Field::JvmArgs => self
                        .draft
                        .as_ref()
                        .map(|draft| draft.jvm_args.clone())
                        .unwrap_or_default(),
                    Field::MemoryMinGlobal => self.memory_min.clone(),
                    Field::JvmGlobal => self.jvm_args.clone(),
                    Field::InstanceName => String::new(),
                };
                self.begin_edit(field, current);
                Effect::None
            }
            SettingKind::UseJava(java_index) => {
                if let Some(java) = self.detected.get(java_index) {
                    self.java_path = java.path.clone();
                    self.note(format!("Java set to {}", java.path), false);
                }
                Effect::Save
            }
            SettingKind::SignIn => Effect::Login,
            SettingKind::Save => Effect::Save,
        }
    }

    fn begin_edit(&mut self, field: Field, value: String) {
        let cursor = value.chars().count();
        self.editing = Some(Editor {
            field,
            origin: value.clone(),
            buffer: value,
            cursor,
        });
    }

    fn on_edit_key(&mut self, key: Key) -> Effect {
        match key {
            Key::Esc => {
                if self
                    .editing
                    .as_ref()
                    .is_some_and(|editor| editor.field == Field::Search)
                {
                    self.clear_search();
                    return Effect::None;
                }
                self.editing = None;
                Effect::None
            }
            Key::Enter => self.commit_edit(),
            Key::Backspace => {
                self.edit_backspace();
                Effect::None
            }
            Key::Left => {
                if let Some(editor) = &mut self.editing {
                    editor.cursor = editor.cursor.saturating_sub(1);
                }
                Effect::None
            }
            Key::Right => {
                if let Some(editor) = &mut self.editing {
                    let len = editor.buffer.chars().count();
                    editor.cursor = (editor.cursor + 1).min(len);
                }
                Effect::None
            }
            Key::Home => {
                if let Some(editor) = &mut self.editing {
                    editor.cursor = 0;
                }
                Effect::None
            }
            Key::End => {
                if let Some(editor) = &mut self.editing {
                    editor.cursor = editor.buffer.chars().count();
                }
                Effect::None
            }
            Key::Char(ch) if !ch.is_control() => {
                self.edit_insert(ch);
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn edit_insert(&mut self, ch: char) {
        if let Some(editor) = &mut self.editing {
            let at = byte_index(&editor.buffer, editor.cursor);
            editor.buffer.insert(at, ch);
            editor.cursor += 1;
        }
        self.mirror_search();
    }

    fn edit_backspace(&mut self) {
        if let Some(editor) = &mut self.editing {
            if editor.cursor == 0 {
                return;
            }
            editor.cursor -= 1;
            let start = byte_index(&editor.buffer, editor.cursor);
            let end = byte_index(&editor.buffer, editor.cursor + 1);
            editor.buffer.replace_range(start..end, "");
        }
        self.mirror_search();
    }

    fn mirror_search(&mut self) {
        let Some(editor) = &self.editing else {
            return;
        };
        if editor.field != Field::Search {
            return;
        }
        self.query = editor.buffer.clone();
        self.download_selected = 0;
        self.download_scroll = 0;
    }

    fn commit_edit(&mut self) -> Effect {
        let Some(editor) = self.editing.clone() else {
            return Effect::None;
        };
        match editor.field {
            Field::Memory | Field::MemoryMax | Field::MemoryMin | Field::MemoryMinGlobal
                if !editor.buffer.trim().is_empty() && !valid_memory(&editor.buffer) =>
            {
                self.note("Memory should look like 2G or 4096M", true);
                return Effect::None;
            }
            Field::Rename if editor.buffer.trim().is_empty() => {
                self.note("Name cannot be empty", true);
                return Effect::None;
            }
            Field::Username if editor.buffer.trim().is_empty() => {
                self.note("Username cannot be empty", true);
                return Effect::None;
            }
            Field::InstanceName if editor.buffer.trim().is_empty() => {
                self.note("Instance name cannot be empty", true);
                return Effect::None;
            }
            Field::GameDir if editor.buffer.trim().is_empty() => {
                self.note("Game directory cannot be empty", true);
                return Effect::None;
            }
            _ => {}
        }
        self.editing = None;
        match editor.field {
            Field::InstanceName => {
                let name = editor.buffer.trim().to_string();
                let Some(catalog_id) = self.pending_catalog.clone() else {
                    self.note("Choose a downloaded version first", true);
                    return Effect::None;
                };
                self.pending_catalog = None;
                return Effect::CreateNamed { catalog_id, name };
            }
            Field::PackPath => {
                let path = editor.buffer.trim().to_string();
                if path.is_empty() {
                    self.note("Enter the path to a .mrpack file", true);
                    return Effect::None;
                }
                self.creator = None;
                let id = self.start_job("modpack".into(), false, true);
                return Effect::ImportPack { id, path };
            }
            Field::GameDir => {
                self.game_dir = PathBuf::from(editor.buffer.trim());
                self.version_selected = 0;
                self.version_scroll = 0;
                self.reload_installed();
                self.reload_local();
            }
            Field::Memory => self.memory = editor.buffer.trim().to_string(),
            Field::MemoryMinGlobal => self.memory_min = editor.buffer.trim().to_string(),
            Field::JvmGlobal => self.jvm_args = editor.buffer.trim().to_string(),
            Field::JavaOverride | Field::MemoryMin | Field::MemoryMax | Field::JvmArgs => {
                self.write_draft(editor.field, editor.buffer.trim());
                return Effect::PersistDraft;
            }
            Field::Username => {
                self.username = editor.buffer.trim().to_string();
                self.sync_offline_profile();
            }
            Field::JavaPath => {
                self.java_path = editor.buffer.trim().to_string();
            }
            Field::ClientId => self.client_id_input = editor.buffer.trim().to_string(),
            Field::Search => {
                self.query = editor.buffer;
                self.clamp_download();
                return Effect::None;
            }
            Field::Rename | Field::Description => {
                self.write_draft(editor.field, editor.buffer.trim());
                return Effect::PersistDraft;
            }
        }
        self.note("Settings updated", false);
        Effect::Save
    }

    fn write_draft(&mut self, field: Field, value: &str) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        match field {
            Field::Rename => draft.name = value.to_string(),
            Field::Description => draft.description = value.to_string(),
            Field::JavaOverride => draft.java = value.to_string(),
            Field::MemoryMin => draft.memory_min = value.to_string(),
            Field::MemoryMax => draft.memory_max = value.to_string(),
            Field::JvmArgs => draft.jvm_args = value.to_string(),
            _ => {}
        }
    }

    pub fn replace_profiles(&mut self, microsoft: Vec<Profile>) {
        self.sync_offline_profile();
        self.profiles.retain(|profile| profile.offline);
        self.profiles.extend(microsoft);
        if self.profile_selected >= self.profiles.len() {
            self.profile_selected = 0;
        }
    }

    fn sync_offline_profile(&mut self) {
        let profile = Profile {
            name: self.username.clone(),
            detail: "offline".into(),
            offline: true,
            account_uuid: String::new(),
        };
        if self.profiles.first().is_some_and(|item| item.offline) {
            self.profiles[0] = profile;
        } else {
            self.profiles.insert(0, profile);
        }
    }

    fn rebuild_installed_ids(&mut self) {
        self.installed_ids = self.installed.iter().map(|item| item.id.clone()).collect();
    }

    fn sort_installed(&mut self) {
        let order: Vec<&str> = self.catalog.iter().map(|entry| entry.id.as_str()).collect();
        self.installed.sort_by_key(|item| {
            order
                .iter()
                .position(|id| *id == item.id)
                .unwrap_or(usize::MAX)
        });
    }

    fn clamp_download(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            self.download_selected = 0;
            self.download_scroll = 0;
            return;
        }
        if self.download_selected >= len {
            self.download_selected = len - 1;
        }
        reveal(
            self.download_selected,
            &mut self.download_scroll,
            self.download_window.max(1),
        );
    }

    fn cancel_job(&mut self) -> Effect {
        let Some(job) = self.jobs.pop() else {
            return Effect::None;
        };
        self.cancelled_jobs.insert(job.id);
        if job.launching {
            self.game_running = false;
            self.stop_requested = true;
            self.note("Stopped", false);
            Effect::StopGame
        } else {
            self.note(format!("Cancelled {}", job.version), false);
            Effect::CancelJob { id: job.id }
        }
    }

    pub fn mark_game_stopped(&mut self) {
        self.stop_requested = true;
        self.game_running = false;
        if let Some(job) = self.jobs.iter().rev().find(|job| job.launching) {
            self.cancelled_jobs.insert(job.id);
        }
        self.jobs.retain(|job| !job.launching);
    }

    pub fn overall_progress(&self) -> (u64, u64) {
        let mut done = 0u64;
        let mut total = 0u64;
        for job in &self.jobs {
            if job.launching {
                continue;
            }
            done += job.completed;
            total += job.total.max(job.completed).max(1);
        }
        (done, total)
    }
}

/// Click targets produced while laying out a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Page(Page),
    Account(usize),
    Version(usize),
    NewInstance,
    Creator(usize),
    RowLaunch(usize),
    RowEdit(usize),
    Category(Category),
    DownloadRow(usize),
    InstallRow(usize),
    LoadersRow(usize),
    LoaderKind(usize),
    LoaderSelect(usize),
    LoaderAction(usize),
    Setting(usize),
    Search,
    SignIn,
    Save,
    Refresh,
    CancelLogin,
    CancelJob,
    OpenDataDir,
}

/// Versions that already have both a json file and a client jar.
pub fn scan_installed(root: &Path) -> Vec<Installed> {
    let dir = root.join("versions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            continue;
        }
        let json_path = entry.path().join(format!("{name}.json"));
        let jar_path = entry.path().join(format!("{name}.jar"));
        if !json_path.is_file() || !jar_path.is_file() {
            continue;
        }
        let kind = std::fs::read(&json_path)
            .ok()
            .and_then(|bytes| weathered_core::version::parse_version(&bytes).ok())
            .and_then(|info| info.version_type)
            .unwrap_or_else(|| "installed".into());
        found.push(Installed { id: name, kind });
    }
    found.sort_by(|left, right| left.id.cmp(&right.id));
    found
}

pub fn percent(done: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    ((done.saturating_mul(100)) / total).min(100) as u16
}

pub fn valid_memory(memory: &str) -> bool {
    let memory = memory.trim();
    let digits = memory.trim_end_matches(['k', 'K', 'm', 'M', 'g', 'G']);
    !digits.is_empty()
        && digits.chars().all(|ch| ch.is_ascii_digit())
        && memory.len() - digits.len() <= 1
}

fn bump(index: &mut usize, len: usize, delta: isize) {
    if len == 0 || delta == 0 {
        return;
    }
    let next = *index as isize + delta;
    *index = next.clamp(0, len as isize - 1) as usize;
}

fn slide(selected: &mut usize, scroll: &mut usize, len: usize, window: usize, delta: isize) {
    if len == 0 || delta == 0 {
        return;
    }
    let next = *selected as isize + delta;
    *selected = next.clamp(0, len as isize - 1) as usize;
    reveal(*selected, scroll, window.max(1));
}

fn reveal(selected: usize, scroll: &mut usize, window: usize) {
    if window == 0 {
        return;
    }
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + window {
        *scroll = selected + 1 - window;
    }
}

fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_parts = left.split(|ch: char| !ch.is_ascii_alphanumeric());
    let mut right_parts = right.split(|ch: char| !ch.is_ascii_alphanumeric());
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(left_part), Some(right_part)) => {
                if left_part.is_empty() && right_part.is_empty() {
                    continue;
                }
                let order = match (left_part.parse::<u64>(), right_part.parse::<u64>()) {
                    (Ok(left_num), Ok(right_num)) => left_num.cmp(&right_num),
                    _ => left_part.cmp(right_part),
                };
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

pub fn entries_from_manifest(
    manifest: &weathered_core::manifest::VersionManifest,
) -> Vec<CatalogEntry> {
    manifest
        .versions
        .iter()
        .map(|entry| CatalogEntry {
            id: entry.id.clone(),
            kind: entry.version_type.clone(),
            released: entry
                .release_time
                .as_deref()
                .and_then(|time| time.split('T').next())
                .unwrap_or("-")
                .to_string(),
        })
        .collect()
}

fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

/// Rectangles for one painted frame. The view fills this, then clicks use it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    pub zones: Vec<Zone>,
    pub header: Rect,
    pub data_path: Rect,
    pub nav: Rect,
    pub body: Rect,
    pub footer: Rect,
    pub tabs: Vec<Rect>,
    pub left: Rect,
    pub right: Rect,
    pub account_list: Rect,
    pub version_list: Rect,
    pub version_rows: Vec<Rect>,
    pub row_launch: Vec<Rect>,
    pub row_edit: Vec<Rect>,
    pub new_instance: Rect,
    pub progress: Rect,
    pub cancel: Rect,
    pub download_tabs: Vec<Rect>,
    pub search: Rect,
    pub download_list: Rect,
    pub download_rows: Vec<Rect>,
    pub install_buttons: Vec<Rect>,
    pub loader_buttons: Vec<Rect>,
    pub settings_list: Rect,
    pub setting_rows: Vec<Rect>,
    pub more_refresh: Rect,
    pub modal: Rect,
    pub modal_cancel: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    pub rect: Rect,
    pub action: Click,
}

pub fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width)
        && y < rect.y.saturating_add(rect.height)
        && rect.width > 0
        && rect.height > 0
}

/// Lay out the current page and record every clickable rectangle.
pub fn layout(app: &App, area: Rect) -> Screen {
    let mut screen = Screen::default();
    if area.width < 40 || area.height < 12 {
        screen.header = area;
        return screen;
    }
    let chunks = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(1),
        ratatui::layout::Constraint::Length(1),
        ratatui::layout::Constraint::Min(8),
        ratatui::layout::Constraint::Length(1),
    ])
    .split(area);
    screen.header = chunks[0];
    let title_w = 20.min(screen.header.width);
    let path_w = screen.header.width.saturating_sub(title_w);
    screen.data_path = Rect::new(
        screen.header.x.saturating_add(title_w),
        screen.header.y,
        path_w,
        1,
    );
    if path_w > 0 {
        screen.zones.push(Zone {
            rect: screen.data_path,
            action: Click::OpenDataDir,
        });
    }
    screen.nav = chunks[1];
    screen.body = chunks[2];
    screen.footer = chunks[3];

    let mut x = screen.nav.x.saturating_add(5);
    for page in [Page::Launch, Page::Download, Page::Settings, Page::More] {
        let width = (page.label().len() as u16).saturating_add(2);
        if x.saturating_add(width) > screen.nav.x.saturating_add(screen.nav.width) {
            break;
        }
        let rect = Rect::new(x, screen.nav.y, width, 1);
        screen.zones.push(Zone {
            rect,
            action: Click::Page(page),
        });
        screen.tabs.push(rect);
        x = x.saturating_add(width).saturating_add(1);
    }

    match app.page {
        Page::Launch => layout_launch(app, &mut screen),
        Page::Download => layout_download(app, &mut screen),
        Page::Settings => layout_settings(app, &mut screen),
        Page::More => layout_more(&mut screen),
        Page::Edit => layout_edit(app, &mut screen),
        Page::Loaders => layout_loaders(app, &mut screen),
    }

    if let Some(editor) = &app.editing
        && matches!(editor.field, Field::InstanceName | Field::PackPath)
    {
        let title = if editor.field == Field::InstanceName {
            "Instance name"
        } else {
            "Modpack path"
        };
        let text = editor.buffer.chars().count().max(title.len()).max(24) as u16;
        let width = text
            .saturating_add(4)
            .min(screen.body.width)
            .max(16.min(screen.body.width));
        let height = 6.min(screen.body.height);
        screen.modal = Rect::new(
            screen.body.x + screen.body.width.saturating_sub(width) / 2,
            screen.body.y + screen.body.height.saturating_sub(height) / 2,
            width,
            height,
        );
    }
    if app.login.is_some() {
        let width = 42.min(screen.body.width);
        let height = 8.min(screen.body.height);
        let rect = Rect::new(
            screen.body.x + screen.body.width.saturating_sub(width) / 2,
            screen.body.y + screen.body.height.saturating_sub(height) / 2,
            width,
            height,
        );
        screen.modal = rect;
        let cancel = Rect::new(
            rect.x.saturating_add(2),
            rect.y.saturating_add(rect.height.saturating_sub(2)),
            10.min(rect.width.saturating_sub(4)),
            1,
        );
        screen.modal_cancel = cancel;
        screen.zones.push(Zone {
            rect: cancel,
            action: Click::CancelLogin,
        });
    }
    screen
}

fn layout_launch(app: &App, screen: &mut Screen) {
    let body = screen.body;
    if body.width >= 78 {
        let left_width = 32.min(body.width / 3);
        screen.left = Rect::new(body.x, body.y, left_width, body.height);
        let right_x = body.x.saturating_add(left_width).saturating_add(1);
        let right_width = body.width.saturating_sub(left_width).saturating_sub(1);
        screen.right = Rect::new(right_x, body.y, right_width, body.height);
    } else {
        let top = body.height.min(8);
        screen.left = Rect::new(body.x, body.y, body.width, top);
        screen.right = Rect::new(
            body.x,
            body.y.saturating_add(top),
            body.width,
            body.height.saturating_sub(top),
        );
    }
    let account_inner = inner(screen.left);
    let card = 4.min(account_inner.height);
    screen.account_list = Rect::new(
        account_inner.x,
        account_inner.y.saturating_add(card),
        account_inner.width,
        account_inner.height.saturating_sub(card),
    );
    for (index, rect) in rows(screen.account_list, app.profiles.len(), 0)
        .into_iter()
        .enumerate()
    {
        screen.zones.push(Zone {
            rect,
            action: Click::Account(index),
        });
    }

    let right_inner = inner(screen.right);
    let progress_h = job_band(app);
    let new_h = if app.creator.is_none() { 1 } else { 0 };
    let list_h = right_inner
        .height
        .saturating_sub(progress_h)
        .saturating_sub(new_h);
    screen.version_list = Rect::new(right_inner.x, right_inner.y, right_inner.width, list_h);
    if new_h > 0 {
        screen.new_instance = Rect::new(
            right_inner.x,
            right_inner.y.saturating_add(list_h),
            right_inner.width,
            1,
        );
        screen.zones.push(Zone {
            rect: screen.new_instance,
            action: Click::NewInstance,
        });
    }
    let list_len = if app.creator.is_some() {
        app.creator_len()
    } else {
        app.instances.len()
    };
    screen.version_rows = rows(screen.version_list, list_len, app.version_scroll);
    screen.row_launch.clear();
    screen.row_edit.clear();
    for (offset, rect) in screen.version_rows.iter().copied().enumerate() {
        if app.creator.is_some() {
            screen.row_launch.push(Rect::default());
            screen.row_edit.push(Rect::default());
            screen.zones.push(Zone {
                rect,
                action: Click::Creator(offset),
            });
            continue;
        }
        let index = app.version_scroll + offset;
        if let Some((launch, edit)) = row_action_rects(rect) {
            let name_w = launch.x.saturating_sub(rect.x);
            screen.zones.push(Zone {
                rect: Rect::new(rect.x, rect.y, name_w, 1),
                action: Click::Version(index),
            });
            screen.zones.push(Zone {
                rect: launch,
                action: Click::RowLaunch(index),
            });
            screen.zones.push(Zone {
                rect: edit,
                action: Click::RowEdit(index),
            });
            screen.row_launch.push(launch);
            screen.row_edit.push(edit);
        } else {
            screen.zones.push(Zone {
                rect,
                action: Click::Version(index),
            });
            screen.row_launch.push(Rect::default());
            screen.row_edit.push(Rect::default());
        }
    }
    if progress_h > 0 {
        place_cancel(
            screen,
            Rect::new(
                right_inner.x,
                right_inner.y.saturating_add(list_h).saturating_add(new_h),
                right_inner.width,
                progress_h,
            ),
        );
    }
}

fn job_band(app: &App) -> u16 {
    if app.jobs.is_empty() {
        0
    } else {
        2 + (app.jobs.len().min(3) as u16)
    }
}

fn download_action_rects(rect: Rect) -> Option<(Rect, Rect)> {
    pair_buttons(rect, 18, 9)
}

fn row_action_rects(rect: Rect) -> Option<(Rect, Rect)> {
    pair_buttons(rect, 8, 6)
}

fn pair_buttons(rect: Rect, left_w: u16, right_w: u16) -> Option<(Rect, Rect)> {
    let gap = 1;
    let total = left_w + gap + right_w;
    if rect.width <= total + 8 {
        return None;
    }
    let right = Rect::new(
        rect.x.saturating_add(rect.width.saturating_sub(right_w)),
        rect.y,
        right_w,
        1,
    );
    let left = Rect::new(
        right.x.saturating_sub(gap).saturating_sub(left_w),
        rect.y,
        left_w,
        1,
    );
    Some((left, right))
}

fn layout_loaders(app: &App, screen: &mut Screen) {
    let body = screen.body;
    screen.download_tabs.clear();
    let mut x = body.x.saturating_add(1);
    for (index, label) in ["Fabric", "Quilt", "Forge", "NeoForge"]
        .into_iter()
        .enumerate()
    {
        let width = (label.len() as u16).saturating_add(2);
        let rect = Rect::new(x, body.y, width, 1);
        screen.zones.push(Zone {
            rect,
            action: Click::LoaderKind(index),
        });
        screen.download_tabs.push(rect);
        x = x.saturating_add(width).saturating_add(1);
    }
    let list = Rect::new(
        body.x,
        body.y.saturating_add(1),
        body.width,
        body.height.saturating_sub(1),
    );
    screen.download_list = inner(list);
    let count = app.picker_visible().len();
    let scroll = app.loader_scroll(screen.download_list.height as usize);
    screen.download_rows = rows(screen.download_list, count, scroll);
    screen.install_buttons.clear();
    for (offset, rect) in screen.download_rows.iter().copied().enumerate() {
        let index = scroll + offset;
        let button_w = 18.min(rect.width / 2);
        if button_w >= 9 {
            let button = Rect::new(
                rect.x.saturating_add(rect.width.saturating_sub(button_w)),
                rect.y,
                button_w,
                1,
            );
            let name_w = button.x.saturating_sub(rect.x);
            screen.zones.push(Zone {
                rect: Rect::new(rect.x, rect.y, name_w, 1),
                action: Click::LoaderSelect(index),
            });
            screen.install_buttons.push(button);
            screen.zones.push(Zone {
                rect: button,
                action: Click::LoaderAction(index),
            });
        } else {
            screen.install_buttons.push(Rect::default());
            screen.zones.push(Zone {
                rect,
                action: Click::LoaderSelect(index),
            });
        }
    }
}

fn layout_download(app: &App, screen: &mut Screen) {
    let body = screen.body;
    let progress_h = job_band(app);
    screen.download_tabs = Vec::new();
    let mut x = body.x.saturating_add(1);
    for category in [Category::Release, Category::Snapshot, Category::Old] {
        let width = (category.label().len() as u16).saturating_add(2);
        let rect = Rect::new(x, body.y, width, 1);
        screen.zones.push(Zone {
            rect,
            action: Click::Category(category),
        });
        screen.download_tabs.push(rect);
        x = x.saturating_add(width).saturating_add(1);
    }
    let search_h = if app.search_open { 1 } else { 0 };
    if search_h > 0 {
        screen.search = Rect::new(
            body.x.saturating_add(1),
            body.y.saturating_add(1),
            body.width.saturating_sub(2),
            1,
        );
        screen.zones.push(Zone {
            rect: screen.search,
            action: Click::Search,
        });
    }
    // Tabs occupy the first row. Search adds a row only while it is open.
    let below_tabs = 1 + search_h;
    let list_y = body.y.saturating_add(below_tabs);
    let list_h = body
        .height
        .saturating_sub(below_tabs)
        .saturating_sub(progress_h);
    let list = Rect::new(body.x, list_y, body.width, list_h);
    screen.download_list = inner(list);
    let count = app.filtered().len();
    screen.download_rows = rows(screen.download_list, count, app.download_scroll);
    screen.install_buttons = Vec::new();
    screen.loader_buttons = Vec::new();
    for (offset, rect) in screen.download_rows.iter().copied().enumerate() {
        let index = app.download_scroll + offset;
        if let Some((action, loaders)) = download_action_rects(rect) {
            let name_w = action.x.saturating_sub(rect.x);
            screen.zones.push(Zone {
                rect: Rect::new(rect.x, rect.y, name_w, 1),
                action: Click::DownloadRow(index),
            });
            screen.install_buttons.push(action);
            screen.loader_buttons.push(loaders);
            screen.zones.push(Zone {
                rect: action,
                action: Click::InstallRow(index),
            });
            screen.zones.push(Zone {
                rect: loaders,
                action: Click::LoadersRow(index),
            });
        } else {
            screen.zones.push(Zone {
                rect,
                action: Click::DownloadRow(index),
            });
            screen.install_buttons.push(Rect::default());
            screen.loader_buttons.push(Rect::default());
        }
    }
    if progress_h > 0 {
        place_cancel(
            screen,
            Rect::new(
                body.x.saturating_add(1),
                body.y
                    .saturating_add(body.height.saturating_sub(progress_h)),
                body.width.saturating_sub(2),
                progress_h,
            ),
        );
    }
}

fn place_cancel(screen: &mut Screen, area: Rect) {
    let cancel_w = 10.min(area.width / 4);
    if cancel_w >= 8 && area.width > cancel_w + 4 {
        screen.cancel = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(cancel_w)),
            area.y,
            cancel_w,
            1,
        );
        screen.progress = Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(cancel_w).saturating_sub(1),
            area.height,
        );
        screen.zones.push(Zone {
            rect: screen.cancel,
            action: Click::CancelJob,
        });
    } else {
        screen.progress = area;
    }
}

fn layout_edit(_app: &App, screen: &mut Screen) {
    screen.settings_list = inner(screen.body);
    screen.setting_rows = rows(screen.settings_list, 9, 0);
    for (index, rect) in screen.setting_rows.iter().copied().enumerate() {
        screen.zones.push(Zone {
            rect,
            action: Click::Setting(index),
        });
    }
}

fn layout_settings(app: &App, screen: &mut Screen) {
    screen.settings_list = inner(screen.body);
    let count = app.setting_rows().len();
    screen.setting_rows = rows(screen.settings_list, count, 0);
    for (index, rect) in screen.setting_rows.iter().copied().enumerate() {
        let action = match app.setting_rows().get(index).map(|row| row.kind) {
            Some(SettingKind::SignIn) => Click::SignIn,
            Some(SettingKind::Save) => Click::Save,
            _ => Click::Setting(index),
        };
        screen.zones.push(Zone { rect, action });
    }
}

fn layout_more(screen: &mut Screen) {
    let inner = inner(screen.body);
    screen.more_refresh = Rect::new(
        inner.x,
        inner.y.saturating_add(inner.height.saturating_sub(1)),
        22.min(inner.width),
        1,
    );
    screen.zones.push(Zone {
        rect: screen.more_refresh,
        action: Click::Refresh,
    });
}

fn inner(rect: Rect) -> Rect {
    if rect.width < 2 || rect.height < 2 {
        return Rect::default();
    }
    Rect::new(
        rect.x.saturating_add(1),
        rect.y.saturating_add(1),
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    )
}

fn rows(area: Rect, len: usize, scroll: usize) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.height == 0 || area.width == 0 || len == 0 {
        return out;
    }
    let start = scroll.min(len.saturating_sub(1));
    for index in 0..area.height as usize {
        let item = start + index;
        if item >= len {
            break;
        }
        out.push(Rect::new(
            area.x,
            area.y.saturating_add(index as u16),
            area.width,
            1,
        ));
    }
    out
}

/// Status text after the game process ends.
pub fn exit_notice(code: i32, log_path: &str, crash_report: &str) -> (String, bool) {
    if code == 0 && crash_report.is_empty() {
        return ("Game exited".into(), false);
    }
    let mut text = if code == 0 {
        "Game crashed.".to_string()
    } else {
        format!("Game crashed (status {code}).")
    };
    if !log_path.is_empty() {
        text.push_str(" Log: ");
        text.push_str(log_path);
    }
    if !crash_report.is_empty() {
        text.push_str(" Crash report: ");
        text.push_str(crash_report);
    }
    (text, true)
}

/// Newest crash report written at or after `since`.
pub fn newest_crash_report(game_dir: &Path, since: std::time::SystemTime) -> Option<PathBuf> {
    let reports = game_dir.join("crash-reports");
    let entries = std::fs::read_dir(reports).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if modified < since {
            continue;
        }
        let replace = best.as_ref().is_none_or(|(time, _)| modified >= *time);
        if replace {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_filter_keeps_old_beta_and_alpha_together() {
        let mut app = App::preview();
        app.category = Category::Old;
        let ids: Vec<_> = app
            .filtered()
            .into_iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["b1.7.3"]);
        app.category = Category::Release;
        app.query = "1.20".into();
        let ids: Vec<_> = app
            .filtered()
            .into_iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["1.20.4"]);
    }

    #[test]
    fn q_and_e_move_pages_while_text_fields_keep_the_letters() {
        let mut app = App::preview();
        app.catalog_saved_at = weathered_core::cache::unix_now();
        app.on_key(Key::Char('e'));
        assert_eq!(app.page, Page::Download);
        app.on_key(Key::Char('s'));
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.download_selected, 1);
        app.on_key(Key::Char('w'));
        assert_eq!(app.download_selected, 0);
        app.on_key(Key::Char('d'));
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.focus, Focus::DownloadLoaders);
        app.on_key(Key::Char('a'));
        assert_eq!(app.focus, Focus::DownloadAction);
        app.on_key(Key::Char('e'));
        assert_eq!(app.page, Page::Settings);
        app.on_key(Key::Char('a'));
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.category, Category::Release);
        app.on_key(Key::Char('q'));
        assert_eq!(app.page, Page::Launch);
        assert_eq!(app.focus, Focus::RowLaunch);
        app.on_key(Key::Char('d'));
        assert_eq!(app.focus, Focus::RowEdit);
        app.on_key(Key::Char('d'));
        assert_eq!(app.focus, Focus::RowEdit);
        app.on_key(Key::Char('a'));
        assert_eq!(app.focus, Focus::RowLaunch);
        app.begin_edit(Field::Rename, String::new());
        app.on_key(Key::Char('q'));
        app.on_key(Key::Char('e'));
        app.on_key(Key::Char('a'));
        app.on_key(Key::Char('d'));
        app.on_key(Key::Char('w'));
        app.on_key(Key::Char('s'));
        let editor = app.editing.clone().unwrap();
        assert_eq!(editor.buffer, "qeadws");
        assert_eq!(app.page, Page::Launch);
    }

    #[test]
    fn startup_focuses_the_default_launch_button() {
        let mut app = App::preview();
        app.apply_startup_focus();
        assert!(app.selected_is_default());
        assert_eq!(app.focus, Focus::RowLaunch);
        assert_eq!(app.on_key(Key::Enter), Effect::Launch);
    }

    #[test]
    fn sideways_keys_move_across_the_row_buttons() {
        let mut app = App::preview();
        app.version_selected = 1;
        app.focus = Focus::RowLaunch;
        app.on_key(Key::Right);
        assert_eq!(app.focus, Focus::RowEdit);
        app.on_key(Key::Right);
        assert_eq!(app.focus, Focus::RowEdit);
        assert!(!app.selected_is_default());
        app.on_key(Key::Left);
        assert_eq!(app.focus, Focus::RowLaunch);
        app.on_key(Key::Up);
        assert_eq!(app.version_selected, 0);
        assert_eq!(app.focus, Focus::RowLaunch);
        app.on_key(Key::Right);
        assert_eq!(app.focus, Focus::RowEdit);
        app.on_key(Key::Left);
        assert_eq!(app.focus, Focus::RowLaunch);
    }

    #[test]
    fn enter_on_the_data_path_opens_the_directory() {
        let mut app = App::preview();
        app.focus = Focus::DataDir;
        assert_eq!(app.on_key(Key::Enter), Effect::OpenDataDir);
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let path = app.screen.data_path;
        assert!(path.width > 0);
        assert_eq!(
            app.on_mouse(Mouse::Click {
                x: path.x + path.width - 1,
                y: path.y,
            }),
            Effect::OpenDataDir
        );
    }

    #[test]
    fn description_edit_shows_each_key_and_escape_cancels_it() {
        let mut app = App::preview();
        app.draft = Some(InstanceDraft {
            id: "vanilla".into(),
            name: "Vanilla".into(),
            description: "dda".into(),
            java: String::new(),
            memory_min: String::new(),
            memory_max: String::new(),
            jvm_args: String::new(),
        });
        app.page = Page::Edit;
        app.setting_selected = 1;
        app.begin_edit(Field::Description, "dda".into());
        app.on_key(Key::Char('x'));
        app.on_key(Key::Left);
        app.on_key(Key::Backspace);
        let editor = app.editing.clone().unwrap();
        assert_eq!(editor.buffer, "ddx");
        assert_eq!(editor.cursor, 2);
        assert_eq!(app.on_key(Key::Esc), Effect::None);
        assert!(app.editing.is_none());
        assert_eq!(app.page, Page::Edit);
        assert_eq!(app.draft.unwrap().description, "dda");
    }

    #[test]
    fn a_crash_names_the_saved_log() {
        let (text, error) = exit_notice(1, "/data/logs/launch-1.log", "");
        assert!(error);
        assert!(text.contains("crashed"));
        assert!(text.contains("/data/logs/launch-1.log"));
        let (clean, error) = exit_notice(0, "/data/logs/launch-1.log", "");
        assert!(!error);
        assert_eq!(clean, "Game exited");
        let root = std::env::temp_dir().join(format!(
            "weathered-crash-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let reports = root.join("crash-reports");
        std::fs::create_dir_all(&reports).unwrap();
        let old = reports.join("old.txt");
        let old_file = std::fs::File::create(&old).unwrap();
        old_file
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120))
            .unwrap();
        drop(old_file);
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(30);
        let fresh = reports.join("crash.txt");
        std::fs::write(&fresh, b"boom").unwrap();
        let found = newest_crash_report(&root, since).unwrap();
        assert_eq!(found, fresh);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ctrl_f_opens_search_and_the_first_escape_clears_it() {
        let mut app = App::preview();
        app.page = Page::Download;
        app.start_job("1.21.1".into(), false, false);
        assert!(!app.search_open);
        app.on_key(Key::CtrlF);
        assert!(app.search_open);
        app.on_key(Key::Char('1'));
        assert_eq!(app.on_key(Key::Esc), Effect::None);
        assert!(!app.search_open);
        assert!(app.query.is_empty());
        assert!(!app.jobs.is_empty());
        assert!(matches!(app.on_key(Key::Esc), Effect::CancelJob { .. }));
    }

    #[test]
    fn edit_page_default_and_open_folder_are_direct_actions() {
        let mut app = App::preview();
        app.draft = Some(InstanceDraft {
            id: "vanilla".into(),
            name: "Vanilla".into(),
            description: String::new(),
            java: String::new(),
            memory_min: String::new(),
            memory_max: String::new(),
            jvm_args: String::new(),
        });
        app.page = Page::Edit;
        app.setting_selected = 2;
        assert_eq!(app.on_key(Key::Enter), Effect::SetDefault);
        assert_eq!(app.page, Page::Edit);
        app.setting_selected = 7;
        assert_eq!(app.on_key(Key::Enter), Effect::OpenFolder);
        assert_eq!(app.page, Page::Edit);
    }

    #[test]
    fn edit_page_delete_has_no_confirmation() {
        let mut app = App::preview();
        app.draft = Some(InstanceDraft {
            id: "vanilla".into(),
            name: "Vanilla".into(),
            description: String::new(),
            java: String::new(),
            memory_min: String::new(),
            memory_max: String::new(),
            jvm_args: String::new(),
        });
        app.page = Page::Edit;
        app.setting_selected = 8;
        assert_eq!(app.on_key(Key::Enter), Effect::DeleteInstance);
        assert_eq!(app.page, Page::Launch);
    }

    #[test]
    fn download_sort_cycles_date_name_and_stored() {
        let mut app = App::preview();
        app.page = Page::Download;
        app.category = Category::Release;
        let ids: Vec<_> = app
            .filtered()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["1.21.1", "1.20.4"]);
        assert_eq!(app.on_key(Key::Char('o')), Effect::Save);
        assert_eq!(app.download_sort, DownloadSort::Oldest);
        let ids: Vec<_> = app
            .filtered()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["1.20.4", "1.21.1"]);
        app.catalog_items[0].game_version = "1.20.4".into();
        app.download_sort = DownloadSort::Stored;
        let ids: Vec<_> = app
            .filtered()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids[0], "1.20.4");
        app.download_sort = DownloadSort::Newest;
        let ids: Vec<_> = app
            .filtered()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids[0], "1.21.1");
        assert_eq!(version_cmp("1.20.4", "1.21.1"), std::cmp::Ordering::Less);
        assert_eq!(version_cmp("26.3", "26.10"), std::cmp::Ordering::Less);
    }

    #[test]
    fn sort_key_cycles_and_asks_to_save() {
        let mut app = App::preview();
        assert_eq!(app.on_key(Key::Char('o')), Effect::Save);
        assert_eq!(app.sort_mode, SortMode::Created);
        assert_eq!(app.on_key(Key::Char('o')), Effect::Save);
        assert_eq!(app.sort_mode, SortMode::Played);
    }

    #[test]
    fn number_keys_switch_pages_and_q_quits() {
        let mut app = App::preview();
        app.catalog_saved_at = weathered_core::cache::unix_now();
        assert_eq!(app.on_key(Key::Char('2')), Effect::None);
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.on_key(Key::Char('q')), Effect::None);
        assert_eq!(app.page, Page::Launch);
        assert_eq!(app.on_key(Key::CtrlC), Effect::Quit);
        assert!(app.quit);
    }

    #[test]
    fn wheel_moves_the_download_selection() {
        let mut app = App::preview();
        app.page = Page::Download;
        app.category = Category::Release;
        app.download_window = 1;
        app.screen.download_list = Rect::new(0, 0, 40, 5);
        app.on_mouse(Mouse::Scroll {
            x: 2,
            y: 1,
            down: true,
        });
        assert_eq!(app.download_selected, 1);
        assert_eq!(app.download_scroll, 1);
    }

    #[test]
    fn progress_updates_without_leaving_the_page() {
        let mut app = App::preview();
        app.page = Page::Download;
        let id = app.start_job("1.20.4".into(), false, false);
        app.on_msg(Msg::Progress {
            id,
            version: "1.20.4".into(),
            stage: "assets".into(),
            completed: 50,
            total: 200,
            detail: "icons/icon.png".into(),
            launching: false,
        });
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.jobs.first().map(Job::percent), Some(25));
    }

    #[test]
    fn invalid_memory_stays_in_the_editor() {
        let mut app = App::preview();
        app.page = Page::Settings;
        app.begin_edit(Field::Memory, "nope".into());
        assert_eq!(app.on_key(Key::Enter), Effect::None);
        assert!(app.editing.is_some());
        assert!(app.status_error);
    }

    #[test]
    fn scan_installed_requires_json_and_jar() {
        let root = std::env::temp_dir().join(format!(
            "weathered-scan-{}-{}",
            std::process::id(),
            std::time::Instant::now().elapsed().as_nanos()
        ));
        let version = root.join("versions").join("1.2.3");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(
            version.join("1.2.3.json"),
            br#"{"id":"1.2.3","type":"release"}"#,
        )
        .unwrap();
        assert!(scan_installed(&root).is_empty());
        std::fs::write(version.join("1.2.3.jar"), b"jar").unwrap();
        let found = scan_installed(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "1.2.3");
        assert_eq!(found[0].kind, "release");
        let mut app = App::empty(root.clone());
        app.reload_installed();
        assert!(app.installed_ids.contains("1.2.3"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clicking_a_tab_and_the_launch_button() {
        let mut app = App::preview();
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let download = app.screen.tabs.get(1).copied().expect("download tab");
        app.on_mouse(Mouse::Click {
            x: download.x + 1,
            y: download.y,
        });
        assert_eq!(app.page, Page::Download);

        app.page = Page::Launch;
        app.focus = Focus::RowLaunch;
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let name = app.screen.version_rows[0];
        let launch = app.screen.row_launch[0];
        let name_effect = app.on_mouse(Mouse::Click {
            x: name.x + 1,
            y: name.y,
        });
        assert_eq!(name_effect, Effect::None);
        assert_eq!(app.focus, Focus::RowLaunch);
        assert!(app.jobs.is_empty());
        let effect = app.on_mouse(Mouse::Click {
            x: launch.x + 1,
            y: launch.y,
        });
        assert_eq!(effect, Effect::Launch);
        assert!(app.jobs.iter().any(|job| job.launching));
    }

    #[test]
    fn clicking_install_selects_that_version() {
        let mut app = App::preview();
        app.page = Page::Download;
        app.category = Category::Release;
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let button = app.screen.install_buttons[1];
        let effect = app.on_mouse(Mouse::Click {
            x: button.x + 1,
            y: button.y,
        });
        assert!(matches!(
            effect,
            Effect::Install {
                version,
                loader,
                loader_version: None,
                create_instance: false,
                ..
            } if version == "1.20.4" && loader == "vanilla"
        ));
    }

    #[test]
    fn escape_cancels_a_running_download_and_unlocks_the_ui() {
        let mut app = App::preview();
        app.page = Page::Download;
        app.catalog_items.clear();
        app.start_job("26.3".into(), false, false);
        assert_eq!(app.on_key(Key::CtrlC), Effect::Quit);
        app.quit = false;
        assert!(!app.jobs.is_empty());
        assert!(matches!(app.on_key(Key::Esc), Effect::CancelJob { .. }));
        assert!(app.jobs.is_empty());
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let button = app.screen.install_buttons[0];
        let effect = app.on_mouse(Mouse::Click {
            x: button.x + 1,
            y: button.y,
        });
        assert!(matches!(effect, Effect::Install { .. }));
        assert_eq!(app.jobs.len(), 1);
        let again = app.on_mouse(Mouse::Click {
            x: app.screen.install_buttons[1].x + 1,
            y: app.screen.install_buttons[1].y,
        });
        assert!(matches!(again, Effect::Install { .. }));
        assert_eq!(app.jobs.len(), 2);
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        assert!(app.screen.cancel.width >= 8);
        let effect = app.on_mouse(Mouse::Click {
            x: app.screen.cancel.x + 1,
            y: app.screen.cancel.y,
        });
        assert!(matches!(effect, Effect::CancelJob { .. }));
        assert_eq!(app.jobs.len(), 1);
    }

    #[test]
    fn uppercase_shortcuts_match_lowercase_outside_text_fields() {
        let mut app = App::preview();
        app.on_key(Key::Char('S'));
        assert_eq!(app.version_selected, 1);
        app.on_key(Key::Char('W'));
        assert_eq!(app.version_selected, 0);
        app.begin_edit(Field::Rename, String::new());
        app.on_key(Key::Char('W'));
        assert_eq!(app.editing.unwrap().buffer, "W");
        assert_eq!(app.version_selected, 0);
    }

    #[test]
    fn new_instance_download_opens_the_download_page() {
        let mut app = App::preview();
        app.catalog_saved_at = weathered_core::cache::unix_now();
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let button = app.screen.new_instance;
        app.on_mouse(Mouse::Click {
            x: button.x + 1,
            y: button.y,
        });
        assert_eq!(app.creator, Some(CreatorStep::Source));
        assert_eq!(app.on_key(Key::Enter), Effect::None);
        assert_eq!(app.creator, None);
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.focus, Focus::DownloadAction);
    }

    #[test]
    fn a_stored_version_asks_for_an_instance_name() {
        let mut app = App::preview();
        app.catalog_saved_at = weathered_core::cache::unix_now();
        app.page = Page::Download;
        app.focus = Focus::DownloadAction;
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        let name = app.screen.download_rows[0];
        assert_eq!(
            app.on_mouse(Mouse::Click {
                x: name.x + 1,
                y: name.y,
            }),
            Effect::None
        );
        assert!(app.jobs.is_empty());
        assert_eq!(app.focus, Focus::DownloadAction);
        let button = app.screen.install_buttons[0];
        assert_eq!(
            app.on_mouse(Mouse::Click {
                x: button.x + 1,
                y: button.y,
            }),
            Effect::None
        );
        let editor = app.editing.clone().unwrap();
        assert_eq!(editor.field, Field::InstanceName);
        assert_eq!(editor.buffer, "1.21.1");
        assert_eq!(
            app.on_key(Key::Enter),
            Effect::CreateNamed {
                catalog_id: "vanilla-1.21.1".into(),
                name: "1.21.1".into(),
            }
        );
    }

    #[test]
    fn loaders_page_downloads_or_creates_from_the_selected_build() {
        let mut app = App::preview();
        app.page = Page::Loaders;
        app.focus = Focus::LoaderButton;
        app.picker = Some(LoaderPicker {
            game_version: "1.21.1".into(),
            loader: "fabric".into(),
            builds: vec![
                LoaderBuildRow {
                    version: "0.16.9".into(),
                    stable: true,
                    recommended: true,
                },
                LoaderBuildRow {
                    version: "0.17.0-beta".into(),
                    stable: false,
                    recommended: false,
                },
            ],
            show_unstable: false,
            selected: 0,
            loading: false,
            error: String::new(),
            create_instance: false,
        });
        assert_eq!(app.picker_visible().len(), 1);
        app.on_key(Key::Char('b'));
        assert_eq!(app.picker_visible().len(), 2);
        app.on_key(Key::Char('b'));
        let effect = app.on_key(Key::Enter);
        assert!(matches!(
            effect,
            Effect::Install {
                loader,
                loader_version: Some(version),
                create_instance: false,
                ..
            } if loader == "fabric" && version == "0.16.9"
        ));
        app.jobs.clear();
        app.catalog_items.push(CatalogRow {
            id: "fabric-1.21.1-0.16.9".into(),
            name: "Fabric 1.21.1".into(),
            kind: "fabric".into(),
            game_version: "1.21.1".into(),
            launchable: true,
            loader_version: "0.16.9".into(),
        });
        assert_eq!(app.on_key(Key::Enter), Effect::None);
        assert_eq!(app.editing.as_ref().unwrap().buffer, "1.21.1-fabric");
        app.on_key(Key::Esc);
        assert!(app.editing.is_none());
        assert_eq!(app.on_key(Key::Esc), Effect::None);
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.focus, Focus::DownloadAction);
    }

    #[test]
    fn a_fresh_manifest_cache_opens_without_another_fetch() {
        let root = std::env::temp_dir().join(format!(
            "weathered-manifest-{}-{}",
            std::process::id(),
            weathered_core::cache::unix_now()
        ));
        let path = weathered_core::paths::DataLayout::new(&root)
            .cache_dir()
            .join("version_manifest_v2.json");
        weathered_core::cache::write(
            &path,
            br#"{"latest":{"release":"1.21.1","snapshot":"24w14a"},"versions":[{"id":"1.21.1","type":"release","url":"https://example.invalid/v.json","time":"2024-08-08T12:00:00+00:00","releaseTime":"2024-08-08T12:00:00+00:00","sha1":"abc","complianceLevel":1}]}"#,
        )
        .unwrap();
        let mut app = App::empty(root.clone());
        app.load_cached_catalog();
        assert!(app.catalog_ready);
        assert_eq!(app.catalog[0].id, "1.21.1");
        assert_eq!(app.on_key(Key::Char('e')), Effect::None);
        assert_eq!(app.page, Page::Download);
        app.catalog_saved_at = 1;
        assert_eq!(
            app.on_key(Key::Char('r')),
            Effect::RefreshCache { force: true }
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
