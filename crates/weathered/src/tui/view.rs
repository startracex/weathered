//! Paint the terminal UI.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::model::{contains, layout, App, Category, Field, Focus, Page, SettingKind};
use super::theme::{self, ACCENT, BG, BORDER, LAUNCH, ON_ACCENT, TEXT};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.screen = layout(app, area);
    app.version_window = app.screen.version_rows.len().max(1);
    app.download_window = app.screen.download_rows.len().max(1);

    frame.render_widget(Block::default().style(theme::base()), area);
    if area.width < 40 || area.height < 12 {
        let message = Paragraph::new("Resize the terminal to at least 40×12.  Ctrl+C quits.")
            .style(theme::base())
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(message, area);
        return;
    }

    draw_header(frame, app);
    draw_nav(frame, app);
    match app.page {
        Page::Launch => draw_launch(frame, app),
        Page::Download => draw_download(frame, app),
        Page::Settings => draw_settings(frame, app),
        Page::More => draw_more(frame, app),
        Page::Edit => draw_edit(frame, app),
    }
    draw_footer(frame, app);
    if app.screen.cancel.width > 0 {
        let style = if hot(app, app.screen.cancel) {
            theme::selected()
        } else {
            Style::default().fg(ON_ACCENT).bg(ACCENT)
        };
        frame.render_widget(
            Paragraph::new(center("Cancel", app.screen.cancel.width as usize)).style(style),
            app.screen.cancel,
        );
    }
    if app.picker.is_some() {
        draw_picker(frame, app);
    }
    if app.login.is_some() {
        draw_login(frame, app);
    }
}

fn draw_header(frame: &mut Frame, app: &App) {
    let width = app.screen.header.width as usize;
    let path = app.screen.header.width.saturating_sub(28) as usize;
    let dir = shorten(&app.game_dir.display().to_string(), path);
    let left = " weathered  launcher";
    let gap = width.saturating_sub(left.chars().count() + dir.chars().count());
    let line = Line::from(vec![
        Span::styled(
            " weathered",
            Style::default()
                .fg(ACCENT)
                .bg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  launcher", theme::muted()),
        Span::styled(" ".repeat(gap), theme::base()),
        Span::styled(dir, theme::muted()),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::base()), app.screen.header);
}

fn draw_nav(frame: &mut Frame, app: &App) {
    frame.render_widget(Block::default().style(theme::base()), app.screen.nav);
    frame.render_widget(
        Paragraph::new("[Q]").style(theme::muted()),
        Rect::new(app.screen.nav.x, app.screen.nav.y, 3, 1),
    );
    if app.screen.nav.width > 4 {
        frame.render_widget(
            Paragraph::new("[E]").style(theme::muted()),
            Rect::new(
                app.screen
                    .nav
                    .x
                    .saturating_add(app.screen.nav.width.saturating_sub(4)),
                app.screen.nav.y,
                3,
                1,
            ),
        );
    }
    for (index, page) in [Page::Launch, Page::Download, Page::Settings, Page::More]
        .into_iter()
        .enumerate()
    {
        let Some(rect) = app.screen.tabs.get(index).copied() else {
            continue;
        };
        let active = app.page == page || (app.page == Page::Edit && page == Page::Launch);
        let hovered = hot(app, rect) && !active;
        let style = if active {
            theme::selected()
        } else if hovered {
            theme::hover()
        } else {
            theme::base()
        };
        let label = format!(" {} ", page.label());
        frame.render_widget(Paragraph::new(label).style(style), rect);
    }
}

fn draw_footer(frame: &mut Frame, app: &App) {
    let style = if app.status_error {
        theme::danger()
    } else {
        theme::muted()
    };
    let hints = if app.job.is_some() {
        "esc cancel   Ctrl+C quit"
    } else {
        "[Q]/[E] pages  [W]/[S] move  [A]/[D] sideways  Ctrl+F search  O sort  Ctrl+C quit"
    };
    let width = app.screen.footer.width as usize;
    let text = if app.status.is_empty() {
        hints.to_string()
    } else {
        let combined = format!(" {}  ·  {hints}", app.status);
        if combined.chars().count() <= width {
            combined
        } else {
            format!(" {}", app.status)
        }
    };
    frame.render_widget(
        Paragraph::new(fit(&text, width)).style(style),
        app.screen.footer,
    );
}

fn draw_launch(frame: &mut Frame, app: &App) {
    frame.render_widget(panel("Account"), app.screen.left);
    frame.render_widget(panel("Instances"), app.screen.right);
    draw_account_card(frame, app);

    if app.creator.is_some() {
        draw_creator(frame, app);
    } else {
        for (offset, rect) in app.screen.version_rows.iter().copied().enumerate() {
            let index = app.version_scroll + offset;
            let Some(instance) = app.instances.get(index) else {
                continue;
            };
            let selected = index == app.version_selected;
            let style = row_style(selected, hot(app, rect));
            let mark = if selected { "▸" } else { " " };
            let pin = if instance.id == app.default_instance {
                "*"
            } else {
                " "
            };
            let launch = app
                .screen
                .row_launch
                .get(offset)
                .copied()
                .unwrap_or_default();
            let edit = app.screen.row_edit.get(offset).copied().unwrap_or_default();
            let reserved = if launch.width > 0 {
                rect.width.saturating_sub(launch.x.saturating_sub(rect.x))
            } else {
                0
            };
            let label = format!(
                " {mark}{pin} {}  {}  {}",
                instance.name, instance.game_version, instance.loader
            );
            frame.render_widget(
                Paragraph::new(fit(&label, rect.width.saturating_sub(reserved) as usize))
                    .style(style),
                rect,
            );
            paint_row_button(
                frame,
                launch,
                "[Launch]",
                (app.focus == Focus::RowLaunch && selected) || hot(app, launch),
            );
            paint_row_button(
                frame,
                edit,
                "[Edit]",
                (app.focus == Focus::RowEdit && selected) || hot(app, edit),
            );
        }
        if app.instances.is_empty() {
            let note = Rect::new(
                app.screen.version_list.x,
                app.screen.version_list.y,
                app.screen.version_list.width,
                1,
            );
            frame.render_widget(
                Paragraph::new("  No instances yet.").style(theme::muted().bg(theme::SURFACE)),
                note,
            );
        }
        if app.screen.new_instance.height > 0 {
            let style = if hot(app, app.screen.new_instance) {
                theme::selected()
            } else {
                theme::hover()
            };
            frame.render_widget(
                Paragraph::new(fit(
                    " + New instance",
                    app.screen.new_instance.width as usize,
                ))
                .style(style),
                app.screen.new_instance,
            );
        }
    }

    if app.screen.launch_button.height > 0 {
        let running = app.game_running;
        let preparing = app.job.as_ref().is_some_and(|job| job.launching) && !running;
        if app.screen.launch_button.y > app.screen.version_list.y {
            let caption = match app.selected_instance() {
                Some(instance) => format!(
                    "{}   ·   {}   ·   {}",
                    instance.name, instance.game_version, app.memory
                ),
                None => "Create an instance to play".into(),
            };
            let width = app.screen.right.width.saturating_sub(2);
            let x = app.screen.right.x.saturating_add(1);
            frame.render_widget(
                Paragraph::new(center(&caption, width as usize))
                    .style(theme::muted().bg(theme::SURFACE)),
                Rect::new(x, app.screen.launch_button.y.saturating_sub(1), width, 1),
            );
        }
        let (label, style) = if running {
            ("STOP", theme::selected())
        } else if preparing {
            ("PREPARING", theme::disabled_button())
        } else if app.focus == Focus::LaunchButton || hot(app, app.screen.launch_button) {
            (
                "LAUNCH",
                theme::launch_button().add_modifier(Modifier::REVERSED),
            )
        } else {
            ("LAUNCH", theme::launch_button())
        };
        paint_button(frame, app.screen.launch_button, label, style);
    }

    if let Some(job) = &app.job {
        draw_job(
            frame,
            app.screen.progress,
            &job.stage,
            job.percent(),
            &job.detail,
        );
    }
}

fn paint_row_button(frame: &mut Frame, rect: Rect, label: &str, active: bool) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let style = if active {
        theme::selected()
    } else {
        Style::default().fg(ON_ACCENT).bg(ACCENT)
    };
    frame.render_widget(
        Paragraph::new(fit(label, rect.width as usize)).style(style),
        rect,
    );
}

fn draw_edit(frame: &mut Frame, app: &App) {
    frame.render_widget(panel("Edit instance"), app.screen.body);
    let draft = app.draft.as_ref();
    let rows = [
        ("Name", draft.map(|item| item.name.as_str()).unwrap_or("")),
        (
            "Description",
            draft.map(|item| item.description.as_str()).unwrap_or(""),
        ),
        (
            "Default",
            if draft.is_some_and(|item| item.id == app.default_instance) {
                "this instance"
            } else {
                "set as default"
            },
        ),
        ("Java", ""),
        ("Memory min", ""),
        ("Memory max", ""),
        ("JVM args", ""),
        ("Open folder", "file manager"),
        ("Delete", "remove this instance"),
    ];
    for (index, (label, value)) in rows.iter().enumerate() {
        let Some(rect) = app.screen.setting_rows.get(index).copied() else {
            continue;
        };
        let shown = if let Some(text) = live_edit(app, index) {
            text
        } else {
            match index {
                3 => inherit_label(
                    draft.map(|item| item.java.as_str()).unwrap_or(""),
                    &app.java_path,
                    "auto",
                ),
                4 => inherit_label(
                    draft.map(|item| item.memory_min.as_str()).unwrap_or(""),
                    &app.memory_min,
                    "unset",
                ),
                5 => inherit_label(
                    draft.map(|item| item.memory_max.as_str()).unwrap_or(""),
                    &app.memory,
                    "2G",
                ),
                6 => inherit_label(
                    draft.map(|item| item.jvm_args.as_str()).unwrap_or(""),
                    &app.jvm_args,
                    "none",
                ),
                _ => (*value).to_string(),
            }
        };
        let selected = index == app.setting_selected;
        let style = row_style(selected, hot(app, rect));
        let line = format!("  {:<14} {shown}", label);
        frame.render_widget(
            Paragraph::new(fit(&line, rect.width as usize)).style(style),
            rect,
        );
    }
}

fn live_edit(app: &App, index: usize) -> Option<String> {
    let editor = app.editing.as_ref()?;
    let field = match index {
        0 => Field::Rename,
        1 => Field::Description,
        3 => Field::JavaOverride,
        4 => Field::MemoryMin,
        5 => Field::MemoryMax,
        6 => Field::JvmArgs,
        _ => return None,
    };
    if editor.field != field {
        return None;
    }
    Some(with_cursor(&editor.buffer, editor.cursor))
}

fn with_cursor(buffer: &str, cursor: usize) -> String {
    let mut text = String::new();
    let mut placed = false;
    for (index, ch) in buffer.chars().enumerate() {
        if index == cursor {
            text.push('█');
            placed = true;
        }
        text.push(ch);
    }
    if !placed {
        text.push('█');
    }
    text
}

fn inherit_label(override_value: &str, global: &str, fallback: &str) -> String {
    if override_value.trim().is_empty() {
        let shown = if global.trim().is_empty() {
            fallback
        } else {
            global
        };
        format!("inherit global ({shown})")
    } else {
        override_value.to_string()
    }
}

fn draw_picker(frame: &mut Frame, app: &App) {
    let Some(picker) = &app.picker else {
        return;
    };
    let area = app.screen.body;
    let width = area.width.clamp(24, 56);
    let height = area.height.clamp(8, 16);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(panel(&format!(" {} ", picker.loader)), rect);
    let inner = inner_of(rect);
    let toggle = if picker.show_unstable {
        "[x] unstable"
    } else {
        "[ ] unstable"
    };
    let heading = if picker.loading {
        "Loading versions..."
    } else if !picker.error.is_empty() {
        picker.error.as_str()
    } else {
        toggle
    };
    frame.render_widget(
        Paragraph::new(fit(heading, inner.width as usize)).style(theme::muted().bg(theme::SURFACE)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let visible: Vec<_> = picker
        .builds
        .iter()
        .filter(|build| picker.show_unstable || build.stable)
        .collect();
    for (index, build) in visible
        .iter()
        .enumerate()
        .take(inner.height.saturating_sub(1) as usize)
    {
        let y = inner.y.saturating_add(1 + index as u16);
        let selected = index == picker.selected;
        let mark = if build.recommended { "*" } else { " " };
        let label = format!(" {mark} {}", build.version);
        let style = if selected {
            theme::selected()
        } else {
            theme::surface()
        };
        frame.render_widget(
            Paragraph::new(fit(&label, inner.width as usize)).style(style),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
}

fn draw_creator(frame: &mut Frame, app: &App) {
    let title = match app.creator {
        Some(super::model::CreatorStep::Pick) => "Pick downloaded",
        _ => "New instance",
    };
    let heading = Rect::new(
        app.screen.version_list.x,
        app.screen.version_list.y,
        app.screen.version_list.width,
        1,
    );
    if app.screen.version_rows.is_empty() {
        frame.render_widget(
            Paragraph::new(fit(title, heading.width as usize))
                .style(theme::muted().bg(theme::SURFACE)),
            heading,
        );
    }
    for (offset, rect) in app.screen.version_rows.iter().copied().enumerate() {
        let selected = offset == app.creator_selected;
        let style = row_style(selected, hot(app, rect));
        let mark = if selected { "▸" } else { " " };
        let label = match app.creator {
            Some(super::model::CreatorStep::Source) => match offset {
                0 => "From downloaded",
                1 => "Download new",
                _ => "Import modpack",
            },
            Some(super::model::CreatorStep::Pick) => app
                .catalog_items
                .get(offset)
                .map(|item| item.name.as_str())
                .unwrap_or(""),
            None => "",
        };
        frame.render_widget(
            Paragraph::new(fit(&format!(" {mark} {label}"), rect.width as usize)).style(style),
            rect,
        );
    }
}

fn draw_account_card(frame: &mut Frame, app: &App) {
    let left = inner_of(app.screen.left);
    if left.height == 0 {
        return;
    }
    let profile = app.selected_profile();
    let name = profile.map(|item| item.name.as_str()).unwrap_or("Player");
    let detail = profile
        .map(|item| item.detail.as_str())
        .unwrap_or("offline");
    let letter = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_ascii_uppercase()
        .to_string();
    let badge = Rect::new(left.x, left.y, 3.min(left.width), 1);
    frame.render_widget(
        Paragraph::new(format!(" {letter} ")).style(theme::selected()),
        badge,
    );
    let text_x = left.x.saturating_add(4);
    let text_w = left.width.saturating_sub(4);
    frame.render_widget(
        Paragraph::new(fit(name, text_w as usize)).style(
            Style::default()
                .fg(TEXT)
                .bg(theme::SURFACE)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(text_x, left.y, text_w, 1),
    );
    frame.render_widget(
        Paragraph::new(fit(detail, text_w as usize)).style(theme::muted().bg(theme::SURFACE)),
        Rect::new(text_x, left.y.saturating_add(1), text_w, 1),
    );
    frame.render_widget(
        Paragraph::new(" Profiles").style(theme::muted().bg(theme::SURFACE)),
        Rect::new(left.x, left.y.saturating_add(3), left.width, 1),
    );
    let visible = app.screen.account_list.height as usize;
    for index in 0..visible.min(app.profiles.len()) {
        let Some(profile) = app.profiles.get(index) else {
            continue;
        };
        let rect = Rect::new(
            app.screen.account_list.x,
            app.screen.account_list.y.saturating_add(index as u16),
            app.screen.account_list.width,
            1,
        );
        let selected = index == app.profile_selected;
        let style = row_style(selected, hot(app, rect));
        let mark = if selected { "▸" } else { " " };
        let line = format!(" {mark} {:<14} {}", profile.name, profile.detail);
        frame.render_widget(
            Paragraph::new(fit(&line, rect.width as usize)).style(style),
            rect,
        );
    }
}

fn draw_download(frame: &mut Frame, app: &App) {
    for (index, category) in [
        Category::Release,
        Category::Snapshot,
        Category::Old,
        Category::Fabric,
        Category::Quilt,
        Category::Forge,
        Category::NeoForge,
    ]
    .into_iter()
    .enumerate()
    {
        let Some(rect) = app.screen.download_tabs.get(index).copied() else {
            continue;
        };
        let active = app.category == category;
        let style = if active {
            theme::selected()
        } else if hot(app, rect) {
            theme::hover()
        } else {
            theme::base()
        };
        frame.render_widget(
            Paragraph::new(format!(" {} ", category.label())).style(style),
            rect,
        );
    }
    let query = if let Some(editor) = &app.editing {
        if editor.field == Field::Search {
            editor.buffer.as_str()
        } else {
            app.query.as_str()
        }
    } else {
        app.query.as_str()
    };
    if app.search_open {
        let search = format!(" search  {query}");
        let search_style =
            if matches!(app.focus, super::model::Focus::Search) || hot(app, app.screen.search) {
                theme::hover()
            } else {
                theme::base()
            };
        frame.render_widget(
            Paragraph::new(fit(&search, app.screen.search.width as usize)).style(search_style),
            app.screen.search,
        );
    }
    let columns = Rect::new(
        app.screen.download_list.x,
        app.screen.download_list.y.saturating_sub(1),
        app.screen.download_list.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(fit(
            "  version               released      status",
            columns.width as usize,
        ))
        .style(theme::muted()),
        columns,
    );

    let outer = expand(app.screen.download_list);
    if outer.height > 0 {
        frame.render_widget(panel("Versions"), outer);
    }
    let filtered = app.filtered();
    for (offset, rect) in app.screen.download_rows.iter().copied().enumerate() {
        let index = app.download_scroll + offset;
        let Some(entry) = filtered.get(index) else {
            continue;
        };
        let selected = index == app.download_selected;
        let style = row_style(selected, hot(app, rect) && !button_hot(app, offset));
        let status = if app.in_store(&entry.id) {
            "in store"
        } else {
            "not stored"
        };
        let mark = if selected { "▸" } else { " " };
        let label = format!(" {mark} {:<20} {:<12} {}", entry.id, entry.released, status);
        frame.render_widget(
            Paragraph::new(fit(&label, rect.width as usize)).style(style),
            rect,
        );
        if let Some(button) = app.screen.install_buttons.get(offset).copied() {
            let busy = app.job.as_ref().is_some_and(|job| job.version == entry.id);
            let caption = if busy {
                "wait"
            } else if app.in_store(&entry.id) {
                "reinstall"
            } else {
                "install"
            };
            let button_style = if hot(app, button) && !busy {
                theme::selected()
            } else if busy {
                theme::disabled_button()
            } else {
                Style::default().fg(ON_ACCENT).bg(ACCENT)
            };
            frame.render_widget(
                Paragraph::new(fit(&format!(" {caption} "), button.width as usize))
                    .style(button_style),
                button,
            );
        }
    }
    if !app.catalog_ready {
        frame.render_widget(
            Paragraph::new("  Fetching Mojang's version manifest...")
                .style(theme::muted().bg(theme::SURFACE)),
            app.screen.download_list,
        );
    } else if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("  No versions match.").style(theme::muted().bg(theme::SURFACE)),
            app.screen.download_list,
        );
    }
    if let Some(job) = &app.job {
        draw_job(
            frame,
            app.screen.progress,
            &job.stage,
            job.percent(),
            &job.detail,
        );
    }
}

fn draw_settings(frame: &mut Frame, app: &App) {
    frame.render_widget(panel("Settings"), app.screen.body);
    let rows = app.setting_rows();
    for (index, rect) in app.screen.setting_rows.iter().copied().enumerate() {
        let Some(row) = rows.get(index) else {
            continue;
        };
        let selected =
            app.setting_selected == index && matches!(app.focus, super::model::Focus::Settings(_));
        let style = row_style(selected, hot(app, rect));
        let value = if let Some(editor) = &app.editing {
            if let SettingKind::Edit(field) = row.kind {
                if editor.field == field {
                    with_cursor(&editor.buffer, editor.cursor)
                } else {
                    row.value.clone()
                }
            } else {
                row.value.clone()
            }
        } else {
            row.value.clone()
        };
        let mark = if row.selected_mark { "●" } else { " " };
        let line = match row.kind {
            SettingKind::SignIn | SettingKind::Save => {
                format!("  {mark} {}", row.value)
            }
            SettingKind::UseJava(_) => format!("  {mark} {}", row.value),
            SettingKind::Edit(_) => format!("  {:<20} {}", row.label, value),
        };
        frame.render_widget(
            Paragraph::new(fit(&line, rect.width as usize)).style(style),
            rect,
        );
    }
}

fn draw_more(frame: &mut Frame, app: &App) {
    frame.render_widget(panel("More"), app.screen.body);
    let inner = inner_of(app.screen.body);
    let text = vec![
        Line::from(Span::styled(
            "Weathered Launcher",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from("A Minecraft: Java Edition launcher, named for weathered copper."),
        Line::from(""),
        Line::from("Launch selects an instance. New instance copies mods into that instance."),
        Line::from("Download stores vanilla, Fabric, or Quilt once in the shared store."),
        Line::from("Settings stores the game directory, memory, Java, and username."),
        Line::from(""),
        Line::from("Mouse: click tabs, rows, and buttons. The wheel scrolls lists."),
        Line::from("Keys: Q/E pages, W/S move, A/D sideways, Enter activates, Esc goes back."),
        Line::from("Ctrl+F searches downloads. O cycles instance sort. Ctrl+C quits."),
        Line::from(""),
        Line::from("Microsoft login uses a device code. Set an Azure client id first."),
    ];
    let text_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(2),
    );
    frame.render_widget(
        Paragraph::new(text)
            .style(theme::surface())
            .wrap(Wrap { trim: true }),
        text_area,
    );
    let style = if hot(app, app.screen.more_refresh) {
        theme::selected()
    } else {
        Style::default().fg(ON_ACCENT).bg(ACCENT)
    };
    frame.render_widget(
        Paragraph::new(" Refresh versions ").style(style),
        app.screen.more_refresh,
    );
}

fn draw_login(frame: &mut Frame, app: &App) {
    let Some(login) = &app.login else {
        return;
    };
    frame.render_widget(panel("Microsoft sign-in"), app.screen.modal);
    let inner = inner_of(app.screen.modal);
    let lines = vec![
        Line::from("Open this page and enter the code:"),
        Line::from(Span::styled(login.url.clone(), Style::default().fg(ACCENT))),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}  ", login.code),
            Style::default()
                .fg(ON_ACCENT)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(login.message.clone()),
    ];
    let text = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(2),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::surface())
            .wrap(Wrap { trim: true }),
        text,
    );
    let style = if hot(app, app.screen.modal_cancel) {
        theme::selected()
    } else {
        theme::disabled_button()
    };
    frame.render_widget(
        Paragraph::new(" Cancel ").style(style),
        app.screen.modal_cancel,
    );
}

fn draw_job(frame: &mut Frame, rect: Rect, stage: &str, percent: u16, detail: &str) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    let label = format!(" {stage} {percent}%  {detail}");
    frame.render_widget(
        Paragraph::new(fit(&label, rect.width as usize)).style(theme::muted().bg(BG)),
        Rect::new(rect.x, rect.y, rect.width, 1),
    );
    if rect.height > 1 {
        let bar = bar(percent, rect.width as usize);
        frame.render_widget(
            Paragraph::new(bar).style(Style::default().fg(LAUNCH).bg(BG)),
            Rect::new(rect.x, rect.y.saturating_add(1), rect.width, 1),
        );
    }
}

fn paint_button(frame: &mut Frame, rect: Rect, label: &str, style: Style) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(style), rect);
    let mid = rect.y + rect.height / 2;
    frame.render_widget(
        Paragraph::new(center(label, rect.width as usize)).style(style),
        Rect::new(rect.x, mid, rect.width, 1),
    );
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(format!(" {title} "), theme::title()))
        .style(theme::surface())
}

fn row_style(selected: bool, hovered: bool) -> Style {
    if selected {
        theme::selected()
    } else if hovered {
        theme::hover()
    } else {
        theme::surface()
    }
}

fn hot(app: &App, rect: Rect) -> bool {
    app.hover.is_some_and(|(x, y)| contains(rect, x, y))
}

fn button_hot(app: &App, offset: usize) -> bool {
    app.screen
        .install_buttons
        .get(offset)
        .copied()
        .is_some_and(|rect| hot(app, rect))
}

fn inner_of(rect: Rect) -> Rect {
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

fn expand(inner: Rect) -> Rect {
    if inner.width == 0 || inner.height == 0 {
        return Rect::default();
    }
    Rect::new(
        inner.x.saturating_sub(1),
        inner.y.saturating_sub(1),
        inner.width.saturating_add(2),
        inner.height.saturating_add(2),
    )
}

fn fit(text: &str, width: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= width {
            break;
        }
        out.push(ch);
    }
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

fn center(text: &str, width: usize) -> String {
    let len = text.chars().count().min(width);
    let pad = width.saturating_sub(len);
    let left = pad / 2;
    let right = pad - left;
    format!(
        "{}{}{}",
        " ".repeat(left),
        text.chars().take(width).collect::<String>(),
        " ".repeat(right)
    )
}

fn bar(percent: u16, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let filled = (width as u16 * percent.min(100)) / 100;
    let mut out = String::new();
    for index in 0..width {
        if (index as u16) < filled {
            out.push('█');
        } else {
            out.push('░');
        }
    }
    out
}

fn shorten(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max == 0 {
        return text.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail: String = text.chars().rev().take(keep).collect();
    format!("…{}", tail.chars().rev().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn frame_text(buffer: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buffer.area().height {
            for x in 0..buffer.area().width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn launch_page_renders_navigation_account_and_launch_button() {
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains("weathered"), "{text}");
        assert!(text.contains("Launch"), "{text}");
        assert!(text.contains("Download"), "{text}");
        assert!(text.contains("Settings"), "{text}");
        assert!(text.contains("More"), "{text}");
        assert!(text.contains("LAUNCH"), "{text}");
        assert!(text.contains("[Launch]"), "{text}");
        assert!(text.contains("[Edit]"), "{text}");
        assert!(!text.contains("EDIT"), "{text}");
        assert!(text.contains("Steve"), "{text}");
        assert!(text.contains("1.21.1"), "{text}");
        assert!(text.contains("q quit") || text.contains("quit"), "{text}");
    }

    #[test]
    fn download_page_renders_categories() {
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        app.page = Page::Download;
        app.job = Some(super::super::model::Job {
            version: "1.20.4".into(),
            stage: "assets".into(),
            completed: 1200,
            total: 3890,
            detail: "icons/icon.png".into(),
            launching: false,
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains("Release"), "{text}");
        assert!(text.contains("Snapshot"), "{text}");
        assert!(text.contains("install"), "{text}");
        assert!(text.contains("1.21.1"), "{text}");
        assert!(text.contains("assets 30%"), "{text}");
        assert!(text.contains('█'), "{text}");
    }

    #[test]
    fn settings_page_lists_the_launcher_options() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        app.page = Page::Settings;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains("Game directory"), "{text}");
        assert!(text.contains("Memory"), "{text}");
        assert!(text.contains("Username"), "{text}");
        assert!(text.contains("Sign in with Microsoft"), "{text}");
        assert!(text.contains("Save"), "{text}");
    }

    #[test]
    fn edit_page_is_full_screen_and_shows_inherited_settings() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        app.page = Page::Edit;
        app.draft = Some(super::super::model::InstanceDraft {
            id: "vanilla".into(),
            name: "Vanilla".into(),
            description: "world".into(),
            java: String::new(),
            memory_min: String::new(),
            memory_max: String::new(),
            jvm_args: String::new(),
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains("Edit instance"), "{text}");
        assert!(text.contains("inherit global"), "{text}");
        assert!(text.contains("Delete"), "{text}");
        assert!(text.contains("Open folder"), "{text}");
        assert!(!text.contains("Account"), "{text}");
    }

    #[test]
    fn edit_field_shows_the_cursor_while_typing() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        app.page = Page::Edit;
        app.setting_selected = 1;
        app.draft = Some(super::super::model::InstanceDraft {
            id: "vanilla".into(),
            name: "Vanilla".into(),
            description: "dda".into(),
            java: String::new(),
            memory_min: String::new(),
            memory_max: String::new(),
            jvm_args: String::new(),
        });
        app.on_key(super::super::model::Key::Enter);
        app.on_key(super::super::model::Key::Char('x'));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains("ddax█"), "{text}");
    }

    #[test]
    fn big_launch_is_hidden_for_a_non_default_instance() {
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        app.version_selected = 1;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains("[Launch]"), "{text}");
        assert!(text.contains("[Edit]"), "{text}");
        assert!(!text.contains("LAUNCH"), "{text}");
        assert_eq!(app.screen.launch_button.height, 0);
    }

    #[test]
    fn download_search_stays_hidden_until_ctrl_f() {
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::preview();
        app.page = Page::Download;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(app.screen.search.height, 0);
        app.search_open = true;
        app.query = "1.20".into();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let shown = frame_text(terminal.backend().buffer());
        assert!(app.screen.search.height > 0);
        assert!(shown.contains(" search  1.20"), "{shown}");
    }
}
