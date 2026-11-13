//! Terminal UI state, keyboard handling, and mouse hit testing.
//!
//! Drawing lives in [`super::view`]. Background work reports [`Msg`] values
//! so the screen can keep painting while a download or launch is in progress.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

/// Top-level pages, in navigation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Launch,
    Download,
    Settings,
    More,
}

impl Page {
    pub fn index(self) -> usize {
        match self {
            Self::Launch => 0,
            Self::Download => 1,
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
    LaunchButton,
    DownloadCats,
    DownloadList,
    Search,
    Settings(usize),
    More,
}

/// A key the UI understands. Crossterm events are translated at the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
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
    Install { version: String },
    Launch,
    StopGame,
    Save,
    Login,
    CancelLogin,
    RefreshManifest,
    Quit,
}

/// Messages from background tasks.
#[derive(Debug, Clone)]
pub enum Msg {
    Catalog(Vec<CatalogEntry>),
    Notice {
        text: String,
        error: bool,
    },
    Javas(Vec<DetectedJava>),
    Progress {
        version: String,
        stage: String,
        completed: u64,
        total: u64,
        detail: String,
        launching: bool,
    },
    InstallFinished {
        version: String,
        ok: bool,
        detail: String,
    },
    GameLog(String),
    GameExit(i32),
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
    /// The game process is on screen and the Launch button should become Stop.
    GameStarted,
    /// Preparing the launch command failed before the process existed.
    LaunchFailed {
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
    pub version: String,
    pub stage: String,
    pub completed: u64,
    pub total: u64,
    pub detail: String,
    pub launching: bool,
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
    pub job: Option<Job>,
    pub log: Vec<String>,
    pub status: String,
    pub status_error: bool,
    pub editing: Option<Editor>,
    pub login: Option<LoginPrompt>,
    pub game_running: bool,
    pub screen: Screen,
    pub hover: Option<(u16, u16)>,
    pub quit: bool,
    last_version_click: Option<(usize, Instant)>,
}

impl App {
    /// Empty launcher state pointed at `game_dir`.
    pub fn empty(game_dir: PathBuf) -> Self {
        let mut app = Self {
            page: Page::Launch,
            focus: Focus::Versions,
            game_dir,
            memory: "2G".into(),
            username: "Steve".into(),
            java_path: String::new(),
            client_id_input: String::new(),
            detected: Vec::new(),
            installed: Vec::new(),
            installed_ids: HashSet::new(),
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
            job: None,
            log: Vec::new(),
            status: "Ready".into(),
            status_error: false,
            editing: None,
            login: None,
            game_running: false,
            screen: Screen::default(),
            hover: None,
            quit: false,
            last_version_click: None,
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
        app.note("Ready", false);
        app
    }

    pub fn note(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_error = error;
    }

    pub fn push_log(&mut self, line: String) {
        if line.is_empty() {
            return;
        }
        self.log.push(line);
        if self.log.len() > 200 {
            let drop_count = self.log.len() - 200;
            self.log.drain(0..drop_count);
        }
    }

    pub fn filtered(&self) -> Vec<&CatalogEntry> {
        let query = self.query.to_ascii_lowercase();
        self.catalog
            .iter()
            .filter(|entry| self.category.matches(&entry.kind))
            .filter(|entry| query.is_empty() || entry.id.to_ascii_lowercase().contains(&query))
            .collect()
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
                label: "Memory".into(),
                value: self.memory.clone(),
                kind: SettingKind::Edit(Field::Memory),
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
        if !self.installed.is_empty() && self.version_selected >= self.installed.len() {
            self.version_selected = 0;
        }
    }

    pub fn selected_version(&self) -> Option<&Installed> {
        self.installed.get(self.version_selected)
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
        match key {
            Key::Char('q') => {
                self.quit = true;
                Effect::Quit
            }
            Key::Char('1') => self.goto(Page::Launch),
            Key::Char('2') => self.goto(Page::Download),
            Key::Char('3') => self.goto(Page::Settings),
            Key::Char('4') => self.goto(Page::More),
            Key::Char('/') if self.page == Page::Download => {
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
            Msg::Catalog(entries) => {
                self.catalog = entries;
                self.catalog_ready = true;
                self.clamp_download();
                self.sort_installed();
                if self.status == "Loading version list..." || self.status == "Ready" {
                    self.note(format!("{} versions", self.catalog.len()), false);
                }
            }
            Msg::Notice { text, error } => self.note(text, error),
            Msg::Javas(found) => {
                self.detected = found;
                if self.setting_selected >= self.setting_rows().len() {
                    self.setting_selected = 0;
                }
            }
            Msg::Progress {
                version,
                stage,
                completed,
                total,
                detail,
                launching,
            } => {
                self.job = Some(Job {
                    version,
                    stage,
                    completed,
                    total,
                    detail,
                    launching,
                });
            }
            Msg::InstallFinished {
                version,
                ok,
                detail,
            } => {
                if self.job.as_ref().is_some_and(|job| job.version == version) {
                    self.job = None;
                }
                if ok {
                    self.reload_installed();
                    if let Some(index) = self.installed.iter().position(|item| item.id == version) {
                        self.version_selected = index;
                    }
                    self.note(format!("Installed {version}. {detail}"), false);
                } else {
                    self.note(format!("Install {version} failed: {detail}"), true);
                }
            }
            Msg::GameLog(line) => self.push_log(line),
            Msg::GameExit(code) => {
                self.game_running = false;
                self.job = None;
                if code == 0 {
                    self.note("Game exited", false);
                } else {
                    self.note(format!("Game exited with status {code}"), true);
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
            Msg::GameStarted => {
                self.game_running = true;
                self.note("Game is running", false);
            }
            Msg::LaunchFailed { detail } => {
                self.game_running = false;
                if self.job.as_ref().is_some_and(|job| job.launching) {
                    self.job = None;
                }
                self.note(detail, true);
            }
        }
    }

    fn goto(&mut self, page: Page) -> Effect {
        self.page = page;
        self.focus = match page {
            Page::Launch => Focus::Versions,
            Page::Download => Focus::DownloadList,
            Page::Settings => Focus::Settings(self.setting_selected),
            Page::More => Focus::More,
        };
        Effect::None
    }

    fn cycle_focus(&mut self, reverse: bool) {
        self.focus = match (self.page, self.focus, reverse) {
            (Page::Launch, Focus::Versions, false) => Focus::Accounts,
            (Page::Launch, Focus::Accounts, false) => Focus::LaunchButton,
            (Page::Launch, _, false) => Focus::Versions,
            (Page::Launch, Focus::Versions, true) => Focus::LaunchButton,
            (Page::Launch, Focus::LaunchButton, true) => Focus::Accounts,
            (Page::Launch, _, true) => Focus::Versions,
            (Page::Download, Focus::DownloadList, false) => Focus::DownloadCats,
            (Page::Download, Focus::DownloadCats, false) => Focus::Search,
            (Page::Download, _, false) => Focus::DownloadList,
            (Page::Download, Focus::DownloadList, true) => Focus::Search,
            (Page::Download, Focus::Search, true) => Focus::DownloadCats,
            (Page::Download, _, true) => Focus::DownloadList,
            (Page::Settings, _, _) => Focus::Settings(self.setting_selected),
            (Page::More, _, _) => Focus::More,
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
            Page::Launch => {
                self.focus = Focus::Versions;
                slide(
                    &mut self.version_selected,
                    &mut self.version_scroll,
                    self.installed.len(),
                    self.version_window,
                    delta,
                );
            }
            Page::Download if self.focus == Focus::DownloadCats => {
                self.shift_category(delta.signum());
            }
            Page::Download => {
                self.focus = Focus::DownloadList;
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
        }
    }

    fn nudge_side(&mut self, delta: isize) -> Effect {
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
        match self.page {
            Page::Launch if self.focus == Focus::Accounts => Effect::None,
            Page::Launch => self.request_launch(),
            Page::Download if self.focus == Focus::Search => {
                self.begin_edit(Field::Search, self.query.clone());
                Effect::None
            }
            Page::Download if self.focus == Focus::DownloadCats => Effect::None,
            Page::Download => self.request_install(self.download_selected),
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
        self.commit_edit_if_any();
        match action {
            Click::Page(page) => self.goto(page),
            Click::Account(index) => {
                self.page = Page::Launch;
                self.focus = Focus::Accounts;
                self.profile_selected = index;
                Effect::None
            }
            Click::Version(index) => {
                self.page = Page::Launch;
                self.focus = Focus::Versions;
                let now = Instant::now();
                let double = self.last_version_click.is_some_and(|(previous, when)| {
                    previous == index && now.duration_since(when) < Duration::from_millis(400)
                });
                self.version_selected = index;
                self.last_version_click = Some((index, now));
                if double {
                    self.request_launch()
                } else {
                    Effect::None
                }
            }
            Click::LaunchButton => {
                self.focus = Focus::LaunchButton;
                if self.game_running {
                    Effect::StopGame
                } else {
                    self.request_launch()
                }
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
                self.focus = Focus::DownloadList;
                self.download_selected = index;
                Effect::None
            }
            Click::InstallRow(index) => {
                self.page = Page::Download;
                self.focus = Focus::DownloadList;
                self.download_selected = index;
                self.request_install(index)
            }
            Click::Setting(index) => {
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
            Click::Stop => Effect::StopGame,
        }
    }

    fn scroll_at(&mut self, x: u16, y: u16, delta: isize) {
        if contains(self.screen.version_list, x, y) {
            self.focus = Focus::Versions;
            slide(
                &mut self.version_selected,
                &mut self.version_scroll,
                self.installed.len(),
                self.version_window,
                delta,
            );
        } else if contains(self.screen.download_list, x, y) {
            self.focus = Focus::DownloadList;
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

    fn request_launch(&mut self) -> Effect {
        if self.game_running {
            return Effect::StopGame;
        }
        if self.job.is_some() {
            self.note("Wait for the current task to finish", true);
            return Effect::None;
        }
        let Some(version) = self.selected_version().map(|item| item.id.clone()) else {
            self.note("Install a version on the Download page first", true);
            return Effect::None;
        };
        self.job = Some(Job {
            version: version.clone(),
            stage: "preparing".to_string(),
            completed: 0,
            total: 0,
            detail: "Resolving files".into(),
            launching: true,
        });
        self.note(format!("Preparing {version}"), false);
        Effect::Launch
    }

    fn request_install(&mut self, index: usize) -> Effect {
        if self.job.is_some() {
            self.note("A download or launch is already running", true);
            return Effect::None;
        }
        let Some(id) = self.filtered().get(index).map(|entry| entry.id.clone()) else {
            return Effect::None;
        };
        self.download_selected = index;
        self.job = Some(Job {
            version: id.clone(),
            stage: "starting".to_string(),
            completed: 0,
            total: 1,
            detail: "Queued".into(),
            launching: false,
        });
        self.note(format!("Installing {id}"), false);
        Effect::Install { version: id }
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
                    Field::Search => self.query.clone(),
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
                if let Some(editor) = self.editing.clone() {
                    if editor.field == Field::Search {
                        self.query = editor.origin;
                        self.clamp_download();
                    }
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
            let at = byte_index(&editor.buffer, editor.cursor);
            editor.buffer.remove(at);
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
            Field::Memory if !valid_memory(&editor.buffer) => {
                self.note("Memory should look like 2G or 4096M", true);
                return Effect::None;
            }
            Field::Username if editor.buffer.trim().is_empty() => {
                self.note("Username cannot be empty", true);
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
            Field::GameDir => {
                self.game_dir = PathBuf::from(editor.buffer.trim());
                self.version_selected = 0;
                self.version_scroll = 0;
                self.reload_installed();
            }
            Field::Memory => self.memory = editor.buffer.trim().to_string(),
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
        }
        self.note("Settings updated", false);
        Effect::Save
    }

    fn commit_edit_if_any(&mut self) {
        if self.editing.is_some() {
            let _ = self.commit_edit();
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

    pub fn mark_game_stopped(&mut self) {
        self.game_running = false;
        if self.job.as_ref().is_some_and(|job| job.launching) {
            self.job = None;
        }
    }
}

/// Click targets produced while laying out a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Page(Page),
    Account(usize),
    Version(usize),
    LaunchButton,
    Category(Category),
    DownloadRow(usize),
    InstallRow(usize),
    Setting(usize),
    Search,
    SignIn,
    Save,
    Refresh,
    CancelLogin,
    Stop,
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
    pub nav: Rect,
    pub body: Rect,
    pub footer: Rect,
    pub tabs: Vec<Rect>,
    pub left: Rect,
    pub right: Rect,
    pub account_list: Rect,
    pub version_list: Rect,
    pub version_rows: Vec<Rect>,
    pub launch_button: Rect,
    pub progress: Rect,
    pub log: Rect,
    pub download_tabs: Vec<Rect>,
    pub search: Rect,
    pub download_list: Rect,
    pub download_rows: Vec<Rect>,
    pub install_buttons: Vec<Rect>,
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
    screen.nav = chunks[1];
    screen.body = chunks[2];
    screen.footer = chunks[3];

    let mut x = screen.nav.x.saturating_add(1);
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
    }

    if app.login.is_some() {
        let width = screen.body.width.clamp(24, 54);
        let height = screen.body.height.clamp(8, 12);
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
    let show_log = !app.log.is_empty() || app.job.as_ref().is_some_and(|job| job.launching);
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
    let button_band = 7.min(right_inner.height.saturating_sub(3));
    let log_h = if show_log {
        5.min(
            right_inner
                .height
                .saturating_sub(button_band)
                .saturating_sub(3),
        )
    } else {
        0
    };
    let list_h = right_inner
        .height
        .saturating_sub(button_band)
        .saturating_sub(log_h);
    screen.version_list = Rect::new(right_inner.x, right_inner.y, right_inner.width, list_h);
    let band_y = right_inner.y.saturating_add(list_h);
    let band = Rect::new(right_inner.x, band_y, right_inner.width, button_band);
    screen.launch_button = centered(band, 24.min(band.width.saturating_sub(2)), 3);
    if app.game_running {
        screen.zones.push(Zone {
            rect: screen.launch_button,
            action: Click::Stop,
        });
    } else {
        screen.zones.push(Zone {
            rect: screen.launch_button,
            action: Click::LaunchButton,
        });
    }
    screen.version_rows = rows(screen.version_list, app.installed.len(), app.version_scroll);
    for (offset, rect) in screen.version_rows.iter().copied().enumerate() {
        screen.zones.push(Zone {
            rect,
            action: Click::Version(app.version_scroll + offset),
        });
    }
    if log_h > 0 {
        screen.log = Rect::new(
            right_inner.x,
            band.y.saturating_add(band.height),
            right_inner.width,
            log_h,
        );
    }
    if app.job.is_some() {
        screen.progress = Rect::new(
            band.x,
            band.y.saturating_add(band.height.saturating_sub(1)),
            band.width,
            1,
        );
    }
}

fn layout_download(app: &App, screen: &mut Screen) {
    let body = screen.body;
    let progress_h = if app.job.is_some() { 3 } else { 0 };
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
    let list_y = body.y.saturating_add(3);
    let list_h = body.height.saturating_sub(3).saturating_sub(progress_h);
    let list = Rect::new(body.x, list_y, body.width, list_h);
    screen.download_list = inner(list);
    let count = app.filtered().len();
    screen.download_rows = rows(screen.download_list, count, app.download_scroll);
    screen.install_buttons = Vec::new();
    for (offset, rect) in screen.download_rows.iter().copied().enumerate() {
        let index = app.download_scroll + offset;
        screen.zones.push(Zone {
            rect,
            action: Click::DownloadRow(index),
        });
        let button_w = 12.min(rect.width / 3);
        if button_w >= 8 {
            let button = Rect::new(
                rect.x.saturating_add(rect.width.saturating_sub(button_w)),
                rect.y,
                button_w,
                1,
            );
            screen.install_buttons.push(button);
            screen.zones.push(Zone {
                rect: button,
                action: Click::InstallRow(index),
            });
        }
    }
    if progress_h > 0 {
        screen.progress = Rect::new(
            body.x.saturating_add(1),
            body.y
                .saturating_add(body.height.saturating_sub(progress_h)),
            body.width.saturating_sub(2),
            progress_h,
        );
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

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
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
    fn number_keys_switch_pages_and_q_quits() {
        let mut app = App::preview();
        assert_eq!(app.on_key(Key::Char('2')), Effect::None);
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.on_key(Key::Char('q')), Effect::Quit);
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
        app.on_msg(Msg::Progress {
            version: "1.20.4".into(),
            stage: "assets".into(),
            completed: 50,
            total: 200,
            detail: "icons/icon.png".into(),
            launching: false,
        });
        assert_eq!(app.page, Page::Download);
        assert_eq!(app.job.as_ref().map(Job::percent), Some(25));
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
            Instant::now().elapsed().as_nanos()
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
        app.screen = layout(&app, Rect::new(0, 0, 100, 36));
        assert!(app.screen.launch_button.height >= 3);
        assert!(app.screen.launch_button.width >= 20);
        let effect = app.on_mouse(Mouse::Click {
            x: app.screen.launch_button.x + 1,
            y: app.screen.launch_button.y + 1,
        });
        assert_eq!(effect, Effect::Launch);
        assert!(app.job.is_some());
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
        assert_eq!(
            effect,
            Effect::Install {
                version: "1.20.4".into()
            }
        );
    }
}
