use super::{Action, Ui};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use std::collections::VecDeque;

pub(super) enum Page<'a> {
    Actions {
        choices: &'a [Action],
        selected: usize,
    },
    Extras {
        groups: &'a [(String, Vec<String>)],
        checked: &'a [bool],
        cursor: usize,
    },
    Review {
        extras: Option<&'a [String]>,
        seconds: u64,
    },
    Progress(Prompt<'a>),
}

#[derive(Clone, Copy)]
pub(super) enum Prompt<'a> {
    Working,
    Confirm(&'a str, bool, Option<u64>),
    Input(&'a str, &'a str),
    Finished(bool),
}

const TEXT: Color = Color::Rgb(212, 212, 212);
const BRIGHT: Color = Color::Rgb(255, 255, 255);
const DIM: Color = Color::Rgb(154, 154, 154);
const QUIET: Color = Color::Rgb(122, 122, 122);
const FAINT: Color = Color::Rgb(77, 77, 77);
const RAISED: Color = Color::Rgb(48, 48, 48);
const WARN: Color = Color::Rgb(224, 164, 88);

pub(super) const MIN_WIDTH: u16 = 44;
pub(super) const MIN_HEIGHT: u16 = 16;
const WIDE: u16 = 74;

#[derive(Clone, Copy)]
struct Theme(bool);

impl Theme {
    fn fg(self, color: Color) -> Style {
        if self.0 {
            Style::new().fg(color)
        } else {
            Style::default()
        }
    }

    fn strong(self, color: Color) -> Style {
        self.fg(color).add_modifier(Modifier::BOLD)
    }

    fn selection(self) -> Style {
        if self.0 {
            Style::new().fg(BRIGHT).bg(RAISED).bold()
        } else {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }
}

pub(super) struct View<'a> {
    pub action: Action,
    pub source: &'a str,
    pub phase: Option<usize>,
    pub logs: &'a VecDeque<(String, bool)>,
    pub scroll: usize,
    pub title: &'a str,
    pub message: &'a str,
    pub color: bool,
}

impl Ui {
    pub(super) fn draw(&mut self, page: Page<'_>) -> hype_install::Result<()> {
        let view = View {
            action: self.action,
            source: self.source,
            phase: self.phase,
            logs: &self.logs,
            scroll: self.scroll,
            title: &self.title,
            message: &self.message,
            color: self.color,
        };
        self.screen
            .terminal
            .draw(|frame| view.render(frame, page))?;
        Ok(())
    }
}

impl View<'_> {
    pub(super) fn render(&self, frame: &mut Frame<'_>, page: Page<'_>) {
        let theme = Theme(self.color);
        let area = frame.area();
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            frame.render_widget(
                Paragraph::new("Resize to 44 × 16 or press Esc.")
                    .style(theme.fg(TEXT))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let [top, rail, _, body, _, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(if area.width < WIDE { 2 } else { 1 }),
        ])
        .areas(area.inner(Margin::new(2, 1)));

        let steps = self.action.phases().len();
        let (section, progress) = match &page {
            Page::Actions { choices, .. } if !choices.contains(&Action::Install) => {
                ("Update".to_owned(), None)
            }
            Page::Actions { .. } => ("Desktop setup".to_owned(), None),
            Page::Progress(prompt) => {
                let done = self.phase.unwrap_or(0).min(steps);
                let failed = matches!(prompt, Prompt::Finished(true));
                (
                    format!("Step {} of {steps}", (done + 1).min(steps)),
                    Some((done as f64 / steps as f64, failed)),
                )
            }
            _ => (self.action.title().to_owned(), None),
        };
        header(frame, top, rail, theme, &section, progress);

        let keys: Vec<(&str, String)> = match page {
            Page::Actions { choices, selected } => {
                actions(frame, body, theme, choices, selected);
                vec![
                    ("↑↓", "Move".into()),
                    ("Enter", "Choose".into()),
                    ("Esc", "Quit".into()),
                ]
            }
            Page::Extras {
                groups,
                checked,
                cursor,
            } => {
                extras(frame, body, theme, groups, checked, cursor);
                vec![
                    ("Space", "Select".into()),
                    ("a", "All".into()),
                    ("n", "None".into()),
                    ("Enter", "Continue to review".into()),
                    ("Esc", "Back".into()),
                ]
            }
            Page::Review { extras, seconds } => {
                review(frame, body, theme, self.action, self.source, extras);
                vec![
                    ("Enter", start_label(self.action).into()),
                    ("Esc", "Back".into()),
                    ("", format!("Starts by itself in {seconds}s")),
                ]
            }
            Page::Progress(prompt) => {
                self.progress(frame, body, theme, prompt);
                match prompt {
                    Prompt::Confirm(..) => vec![
                        ("y", "Yes".into()),
                        ("n", "No".into()),
                        ("←→ Enter", "Pick and confirm".into()),
                    ],
                    Prompt::Input(..) => {
                        vec![
                            ("Enter", "Send answer".into()),
                            ("Esc", "Stop install".into()),
                        ]
                    }
                    Prompt::Working => {
                        vec![("PgUp/PgDn", "Scroll log".into()), ("End", "Newest".into())]
                    }
                    Prompt::Finished(true) => {
                        vec![
                            ("Enter", "Close".into()),
                            ("PgUp/PgDn", "Scroll log".into()),
                        ]
                    }
                    Prompt::Finished(false) => vec![
                        ("Enter", "Close".into()),
                        ("r", "Reboot now".into()),
                        ("PgUp/PgDn", "Scroll log".into()),
                    ],
                }
            }
        };
        let mut spans = Vec::new();
        for (key, label) in &keys {
            if !spans.is_empty() {
                spans.push(Span::raw("    "));
            }
            if !key.is_empty() {
                spans.push(Span::styled(*key, theme.strong(TEXT)));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(label.as_str(), theme.fg(DIM)));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }),
            footer,
        );
    }

    fn progress(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme, prompt: Prompt<'_>) {
        let failed = matches!(prompt, Prompt::Finished(true));
        let steps = self.action.phases();
        let detail = if area.width >= WIDE {
            let [list, detail] = Layout::horizontal([Constraint::Length(28), Constraint::Min(0)])
                .spacing(3)
                .areas(area);
            if let Some(current) = self.phase {
                let start = current.saturating_sub(list.height.saturating_sub(1) as usize);
                let lines = steps
                    .iter()
                    .enumerate()
                    .skip(start)
                    .map(|(index, (name, _))| {
                        let (mark, style) = if index < current {
                            ("✓", theme.fg(DIM))
                        } else if index == current && failed {
                            ("!", theme.strong(WARN))
                        } else if index == current {
                            ("›", theme.strong(BRIGHT))
                        } else {
                            (" ", theme.fg(QUIET))
                        };
                        Line::styled(format!("{mark} {name}"), style)
                    })
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(lines), list);
            }
            detail
        } else {
            area
        };

        let (heading, message, action) = match prompt {
            Prompt::Working | Prompt::Finished(_) => (self.title, self.message, None),
            Prompt::Confirm(question, yes, seconds) => {
                let option = |text: String, active| {
                    Span::styled(
                        text,
                        if active {
                            theme.selection()
                        } else {
                            theme.fg(DIM)
                        },
                    )
                };
                (
                    "Question",
                    question,
                    Some(Line::from(vec![
                        option(" No ".into(), !yes),
                        Span::raw("  "),
                        option(
                            seconds.map_or_else(|| " Yes ".into(), |s| format!(" Yes in {s}s ")),
                            yes,
                        ),
                    ])),
                )
            }
            Prompt::Input(question, answer) => (
                "Answer needed",
                question,
                Some(Line::styled(format!("› {answer}▏"), theme.strong(BRIGHT))),
            ),
        };
        let mut lines = vec![
            Line::styled(
                heading.trim(),
                theme.strong(if failed { WARN } else { BRIGHT }),
            ),
            Line::raw(""),
        ];
        lines.extend(
            message
                .lines()
                .map(|line| Line::styled(line, theme.fg(TEXT))),
        );
        if let Some(action) = action {
            lines.push(Line::raw(""));
            lines.push(action);
        }
        let width = detail.width.max(1) as usize;
        let height = lines
            .iter()
            .map(|line| line.width().div_ceil(width).max(1))
            .sum::<usize>()
            .min(detail.height as usize) as u16;
        let [status, _, log] = Layout::vertical([
            Constraint::Length(height),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(detail);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), status);

        let [label, log] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(log);
        frame.render_widget(
            Paragraph::new(if self.scroll > 0 {
                "Log, scrolled back. End shows the newest lines."
            } else {
                "Log"
            })
            .style(theme.fg(DIM)),
            label,
        );
        let end = self.logs.len().saturating_sub(self.scroll);
        let start = end.saturating_sub(log.height as usize);
        let rows = self
            .logs
            .range(start..end)
            .map(|(line, warning)| {
                Line::styled(line.as_str(), theme.fg(if *warning { WARN } else { QUIET }))
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(rows), log);
    }
}

fn header(
    frame: &mut Frame<'_>,
    top: Rect,
    rail: Rect,
    theme: Theme,
    section: &str,
    progress: Option<(f64, bool)>,
) {
    let [brand, context] =
        Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(top);
    frame.render_widget(
        Paragraph::new("Hype Niri").style(theme.strong(BRIGHT)),
        brand,
    );
    frame.render_widget(
        Paragraph::new(section).style(theme.fg(DIM)).right_aligned(),
        context,
    );
    let width = rail.width as usize;
    let (filled, color) = progress.map_or((0, BRIGHT), |(ratio, failed)| {
        (
            (ratio * width as f64).round() as usize,
            if failed { WARN } else { BRIGHT },
        )
    });
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("━".repeat(filled), theme.fg(color)),
            Span::styled("─".repeat(width - filled), theme.fg(FAINT)),
        ])),
        rail,
    );
}

fn columns(area: Rect, sidebar: u16) -> [Rect; 2] {
    if area.width >= WIDE {
        Layout::horizontal([Constraint::Length(sidebar), Constraint::Min(0)])
            .spacing(3)
            .areas(area)
    } else {
        let half = (area.height / 2).max(4);
        Layout::vertical([Constraint::Length(half), Constraint::Min(0)])
            .spacing(1)
            .areas(area)
    }
}

fn menu<'a>(
    theme: Theme,
    rows: impl Iterator<Item = Line<'a>>,
    cursor: usize,
    height: u16,
) -> Paragraph<'a> {
    let start = cursor.saturating_sub(height.saturating_sub(1) as usize);
    let lines = rows
        .enumerate()
        .skip(start)
        .map(|(index, line)| {
            if index == cursor {
                line.style(theme.selection())
            } else {
                line
            }
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines)
}

fn actions(frame: &mut Frame<'_>, area: Rect, theme: Theme, choices: &[Action], selected: usize) {
    let [list, detail] = columns(area, 34);
    let rows = choices
        .iter()
        .map(|choice| Line::styled(format!(" {:<32}", choice.label()), theme.fg(TEXT)));
    frame.render_widget(menu(theme, rows, selected, list.height), list);
    let (summary, first, second) = match choices[selected] {
        Action::Install => (
            "Set up the full desktop on this computer.",
            "Installs Niri, Waybar, the notch and the tools they use.",
            "You pick extras and review everything before it starts.",
        ),
        Action::Update => (
            "Update packages and refresh the desktop.",
            "Updates Arch and AUR packages, then copies the latest Hype Niri configs.",
            "Your current configs are backed up first.",
        ),
        Action::Packages => (
            "Update installed software.",
            "Updates Arch and AUR packages.",
            "Your desktop configs are not changed.",
        ),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(summary, theme.strong(BRIGHT)),
            Line::raw(""),
            Line::styled(first, theme.fg(TEXT)),
            Line::styled(second, theme.fg(DIM)),
        ])
        .wrap(Wrap { trim: true }),
        detail,
    );
}

fn extras(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: Theme,
    groups: &[(String, Vec<String>)],
    checked: &[bool],
    cursor: usize,
) {
    let count = checked.iter().filter(|&&check| check).count();
    let [heading, _, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Extras", theme.strong(BRIGHT)),
            Span::styled(
                format!("   {count} of {} selected", groups.len()),
                theme.fg(DIM),
            ),
        ])),
        heading,
    );
    let [list, detail] = columns(body, 30);
    let rows = groups.iter().zip(checked).map(|((name, _), &check)| {
        let (mark, style) = if check {
            ("●", theme.fg(BRIGHT))
        } else {
            ("○", theme.fg(DIM))
        };
        Line::styled(format!(" {mark} {:<26}", group_label(name)), style)
    });
    frame.render_widget(menu(theme, rows, cursor, list.height), list);

    let (name, packages) = &groups[cursor];
    let mut lines = vec![
        Line::styled(group_label(name), theme.strong(BRIGHT)),
        Line::styled(
            if checked[cursor] {
                "Will be installed"
            } else {
                "Will be skipped"
            },
            theme.fg(DIM),
        ),
        Line::raw(""),
        Line::styled(group_note(name), theme.fg(TEXT)),
        Line::raw(""),
        Line::styled("Packages", theme.fg(DIM)),
    ];
    lines.extend(
        packages
            .iter()
            .map(|package| Line::styled(package.as_str(), theme.fg(TEXT))),
    );
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "The core desktop is always installed.",
        theme.fg(DIM),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), detail);
}

fn start_label(action: Action) -> &'static str {
    match action {
        Action::Install => "Start installation",
        Action::Update => "Start update",
        Action::Packages => "Start package update",
    }
}

fn review(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: Theme,
    action: Action,
    source: &str,
    extras: Option<&[String]>,
) {
    let title = match action {
        Action::Install => "Review install",
        Action::Update => "Review update",
        Action::Packages => "Review package update",
    };
    let row = |label: &'static str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<13}"), theme.fg(DIM)),
            Span::styled(value, theme.fg(TEXT)),
        ])
    };
    let mut lines = vec![
        Line::styled(title, theme.strong(BRIGHT)),
        Line::raw(""),
        row("Updates", "Arch packages, then AUR packages".into()),
        row("Password", "sudo asks for it when needed".into()),
    ];
    if let Some(extras) = extras {
        lines.push(row("Backup", "Your current configs are saved first".into()));
        lines.push(row("Files from", source.into()));
        lines.push(row(
            "Extras",
            if extras.is_empty() {
                "None".into()
            } else {
                extras
                    .iter()
                    .map(|name| group_label(name))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Some steps ask questions. Answer them in this window.",
        theme.fg(DIM),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn group_note(name: &str) -> &'static str {
    match name {
        "browser-firefox" | "browser-brave" => "Web browser.",
        "editor-neovim" => "Terminal code editor, set up with LazyVim.",
        "editor-gnome" | "editor-kate" => "Text editor.",
        "editor-code" => "Visual Studio Code editor.",
        "chat" => "Discord chat app.",
        "notes" => "Obsidian notes app.",
        "hype-flow" => "Offline voice typing. Press Alt+Space to dictate.",
        "desktop-tools" => "Color picker, task monitor and volume mixer.",
        "media" => "Video player, screen recorder and video thumbnails.",
        "login-manager-ly" => "Simple login screen that starts Niri.",
        "firewall" => "Blocks incoming connections.",
        "docker" => "Run apps in containers.",
        "bluetooth" => "Bluetooth support and a device manager.",
        "warp" => "Cloudflare's private DNS and optional VPN.",
        "stealth" => "Send all traffic through Tor when you turn it on.",
        "proton-vpn" => "Proton VPN app.",
        "shell-extras" => "Faster terminal tools and a nicer Zsh prompt.",
        "compatibility" => "Support for X11 apps, archives, phones and extra drives.",
        "hardware-intel" => "Graphics and video drivers for Intel GPUs.",
        "theme-extras" => "Grey folder icons and the Bibata cursor.",
        _ => "Optional packages.",
    }
}

fn group_label(name: &str) -> String {
    match name {
        "browser-firefox" => "Firefox".to_owned(),
        "browser-brave" => "Brave".to_owned(),
        "editor-neovim" => "Neovim / LazyVim".to_owned(),
        "editor-gnome" => "GNOME Text Editor".to_owned(),
        "editor-kate" => "Kate".to_owned(),
        "editor-code" => "VS Code".to_owned(),
        "hype-flow" => "Hype Flow dictation".to_owned(),
        "login-manager-ly" => "Ly login screen".to_owned(),
        "desktop-tools" => "Desktop tools".to_owned(),
        "shell-extras" => "Terminal extras".to_owned(),
        "theme-extras" => "Theme extras".to_owned(),
        "chat" => "Discord".to_owned(),
        "notes" => "Obsidian".to_owned(),
        "stealth" => "Stealth routing".to_owned(),
        "warp" => "Cloudflare WARP".to_owned(),
        "proton-vpn" => "Proton VPN".to_owned(),
        "hardware-intel" => "Intel graphics".to_owned(),
        _ => {
            let mut chars = name.chars();
            chars
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .collect::<String>()
                + &chars.as_str().replace('-', " ")
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
    use std::fmt::Write;

    fn ansi(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                let fg = match cell.fg {
                    Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
                    _ => "39".into(),
                };
                let bg = match cell.bg {
                    Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
                    _ => "49".into(),
                };
                let bold = if cell.modifier.contains(Modifier::BOLD) {
                    "1"
                } else {
                    "22"
                };
                write!(out, "\x1b[{bold};{fg};{bg}m{}", cell.symbol()).unwrap();
            }
            out.push_str("\x1b[0m\n");
        }
        out
    }

    #[test]
    #[ignore = "writes ANSI previews to HYPE_TUI_PREVIEW"]
    fn render_pages() {
        let dir = std::env::var("HYPE_TUI_PREVIEW").unwrap();
        let (width, height) = std::env::var("HYPE_TUI_SIZE")
            .ok()
            .and_then(|size| {
                let (w, h) = size.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((100, 30));
        let mut logs = VecDeque::new();
        for line in [
            "Refresh Arch mirrors",
            "Terminal: sudo",
            ":: Synchronizing package databases...",
            " core is up to date",
            " extra is up to date",
            "Installing yay-bin",
        ] {
            logs.push_back((line.to_owned(), false));
        }
        logs.push_back(("warning: ttf-nerd-fonts-symbols is up to date".into(), true));
        let groups = vec![
            ("browser-firefox".to_owned(), vec!["firefox".to_owned()]),
            (
                "editor-neovim".to_owned(),
                vec!["neovim".into(), "lazygit".into(), "ripgrep".into()],
            ),
            (
                "docker".to_owned(),
                vec!["docker".into(), "docker-compose".into()],
            ),
            ("hype-flow".to_owned(), vec!["hype-flow".into()]),
            ("warp".to_owned(), vec!["cloudflare-warp-bin".into()]),
        ];
        let checked = [true, true, false, true, false];
        let names = [
            "browser-firefox".to_owned(),
            "editor-neovim".into(),
            "hype-flow".into(),
        ];
        let pages: Vec<(&str, Option<usize>, &str, &str, Page)> = vec![
            (
                "actions",
                None,
                "",
                "",
                Page::Actions {
                    choices: &[Action::Install, Action::Packages, Action::Update],
                    selected: 0,
                },
            ),
            (
                "extras",
                None,
                "",
                "",
                Page::Extras {
                    groups: &groups,
                    checked: &checked,
                    cursor: 1,
                },
            ),
            (
                "review",
                None,
                "",
                "",
                Page::Review {
                    extras: Some(&names),
                    seconds: 4,
                },
            ),
            (
                "working",
                Some(5),
                "Install Hype Niri",
                "Installing core packages",
                Page::Progress(Prompt::Working),
            ),
            (
                "confirm",
                Some(9),
                "Install Hype Niri",
                "",
                Page::Progress(Prompt::Confirm(
                    "Set up ly as display manager?",
                    false,
                    None,
                )),
            ),
            (
                "failed",
                Some(7),
                "Install failed",
                "pacman failed (exit status: 1)",
                Page::Progress(Prompt::Finished(true)),
            ),
        ];
        for (name, phase, title, message, page) in pages {
            let view = View {
                action: Action::Install,
                source: "This folder",
                phase,
                logs: &logs,
                scroll: 0,
                title,
                message,
                color: true,
            };
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| view.render(frame, page)).unwrap();
            std::fs::write(
                format!("{dir}/{name}.ans"),
                ansi(terminal.backend().buffer()),
            )
            .unwrap();
        }
    }
}
