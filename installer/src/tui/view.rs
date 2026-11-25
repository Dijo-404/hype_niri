use super::{Action, Ui};
use hype_install::core::Phase;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Gauge, List, ListItem, ListState, Padding, Paragraph, Wrap},
};

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

const INK: Color = Color::Rgb(222, 230, 242);
const MUTED: Color = Color::Rgb(160, 177, 198);
const GRAPHITE: Color = Color::Rgb(20, 25, 34);
const SLATE: Color = Color::Rgb(29, 37, 51);
const BORDER: Color = Color::Rgb(82, 101, 128);
const LILAC: Color = Color::Rgb(192, 177, 249);
const MINT: Color = Color::Rgb(139, 219, 200);
const AMBER: Color = Color::Rgb(244, 196, 130);

pub(super) const MIN_WIDTH: u16 = 44;
pub(super) const MIN_HEIGHT: u16 = 16;

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

    fn surface(self, color: Color) -> Style {
        if self.0 {
            self.fg(INK).bg(color)
        } else {
            Style::default()
        }
    }

    fn selection(self) -> Style {
        if self.0 {
            Style::new().fg(GRAPHITE).bg(LILAC).bold()
        } else {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }

    fn panel(self, title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
        Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(self.fg(if focused { LILAC } else { BORDER }))
            .title(title.into().style(self.fg(INK).bold()))
            .padding(Padding::horizontal(1))
            .style(self.surface(SLATE))
    }
}

impl Ui {
    pub(super) fn draw(&mut self, page: Page<'_>) -> hype_install::Result<()> {
        let Self {
            screen,
            action,
            source,
            phase,
            logs,
            scroll,
            title,
            message,
            color,
        } = self;
        let theme = Theme(*color);
        screen.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Block::new().style(theme.surface(GRAPHITE)), area);
            if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
                frame.render_widget(
                    Paragraph::new("Resize to 44 × 16 or press Esc.").wrap(Wrap { trim: true }),
                    area,
                );
                return;
            }
            let [top, body, footer] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .areas(area.inner(Margin::new(1, 0)));
            let section = match &page {
                Page::Actions { choices, .. } if !choices.contains(&Action::Install) => "Update",
                Page::Actions { .. } => "Desktop setup",
                _ => action.title(),
            };
            header(frame, top, theme, section);
            let review_keys;
            let keys = match page {
                Page::Actions { choices, selected } => {
                    actions(frame, body, theme, choices, selected);
                    "↑↓  Choose     Enter  Continue     Esc  Quit"
                }
                Page::Extras {
                    groups,
                    checked,
                    cursor,
                } => {
                    let [instruction, body] =
                        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(body);
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(" Enter ", theme.selection()),
                            Span::raw("  Continue to review"),
                        ])),
                        instruction,
                    );
                    Extras {
                        groups,
                        checked,
                        cursor,
                    }
                    .draw(frame, body, theme);
                    "↑↓  Move     Space  Toggle     a/n  All/none     Esc  Back"
                }
                Page::Review { extras, seconds } => {
                    draw_review(frame, body, theme, *action, source, extras);
                    review_keys = format!("Enter  Start     n / Esc  Back     Yes in {seconds}s");
                    &review_keys
                }
                Page::Progress(prompt) => {
                    let progress = Progress {
                        phase: *phase,
                        steps: action.phases(),
                        logs,
                        scroll: *scroll,
                        title,
                        message,
                        prompt,
                    };
                    progress.draw(frame, body, theme);
                    match prompt {
                        Prompt::Confirm(..) => {
                            "←→ / Tab  Choose     Enter  Confirm     y/n  Answer"
                        }
                        Prompt::Input(..) => "Enter  Submit     Esc  Stop",
                        Prompt::Working => "PgUp/PgDn  Logs     End  Latest",
                        Prompt::Finished(true) => "Enter / q  Close     PgUp/PgDn  Logs",
                        Prompt::Finished(false) => {
                            "Enter / q  Close    r  Reboot    PgUp/PgDn  Logs"
                        }
                    }
                }
            };
            frame.render_widget(
                Paragraph::new(format!(" {keys}"))
                    .style(theme.fg(MUTED))
                    .wrap(Wrap { trim: true }),
                footer,
            );
        })?;
        Ok(())
    }
}

fn header(frame: &mut Frame<'_>, area: Rect, theme: Theme, section: &str) {
    let [bar, _] = Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).areas(area);
    frame.render_widget(Block::new().style(theme.surface(SLATE)), bar);
    let [brand, context] =
        Layout::horizontal([Constraint::Length(15), Constraint::Min(0)]).areas(bar);
    frame.render_widget(
        Paragraph::new("  Hype Niri").style(theme.fg(LILAC).bold()),
        brand,
    );
    frame.render_widget(
        Paragraph::new(format!("Arch Linux / {section}  "))
            .style(theme.fg(MUTED))
            .right_aligned(),
        context,
    );
}

fn columns(area: Rect, stacked: u16, sidebar: u16) -> [Rect; 2] {
    if area.width >= 74 {
        Layout::horizontal([Constraint::Length(sidebar), Constraint::Min(0)])
            .spacing(1)
            .areas(area)
    } else {
        Layout::vertical([Constraint::Length(stacked), Constraint::Min(0)]).areas(area)
    }
}

fn actions(frame: &mut Frame<'_>, area: Rect, theme: Theme, choices: &[Action], selected: usize) {
    let area = Rect {
        height: area.height.min(12),
        ..area
    };
    let [menu, detail] = columns(area, choices.len() as u16 + 2, 37);
    let mut state = ListState::default().with_selected(Some(selected));
    let items = choices.iter().map(|choice| ListItem::new(choice.label()));
    frame.render_stateful_widget(
        List::new(items)
            .block(theme.panel(" Actions ", true))
            .highlight_style(theme.selection())
            .highlight_symbol("› "),
        menu,
        &mut state,
    );
    let (title, explanation, extra) = match choices[selected] {
        Action::Install => (
            "Set up your desktop",
            "Install Niri, Waybar, and the notch.",
            "Review extras before installing.",
        ),
        Action::Update => (
            "System and desktop update",
            "Upgrade Arch and AUR packages.",
            "Refresh Hype Niri and your extras.",
        ),
        Action::Packages => (
            "System package update",
            "Upgrade installed Arch packages.",
            "Upgrade installed AUR packages.",
        ),
    };
    let lines = vec![
        Line::from(title).style(theme.fg(INK).bold()),
        Line::from(explanation),
        Line::from(extra),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(theme.panel(" What changes ", false))
            .wrap(Wrap { trim: true }),
        detail,
    );
}

fn draw_review(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: Theme,
    action: Action,
    source: &str,
    extras: Option<&[String]>,
) {
    let (title, start) = match action {
        Action::Install => ("Review install", "Start installation"),
        Action::Update => ("Review update", "Update packages and Hype Niri"),
        Action::Packages => ("Review package update", "Update system packages"),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Enter ", theme.selection()),
            Span::styled(format!("  {start}"), theme.fg(LILAC).bold()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Arch packages  ", theme.fg(INK).bold()),
            Span::raw("sudo pacman -Syu"),
        ]),
        Line::from(vec![
            Span::styled("AUR packages   ", theme.fg(INK).bold()),
            Span::raw("yay -Syu --aur"),
        ]),
        Line::from("Install yay from the AUR if missing."),
    ];
    if let Some(extras) = extras {
        lines.push(Line::from("Back up configs before replacement."));
        lines.push(
            Line::from(format!("{source} / {} extras selected", extras.len()))
                .style(theme.fg(MUTED)),
        );
        if !extras.is_empty() {
            let names = extras
                .iter()
                .map(|name| group_label(name))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(names).style(theme.fg(MUTED)));
        }
    }
    lines.push(Line::from("Follow the package prompts as they appear.").style(theme.fg(MUTED)));
    frame.render_widget(
        Paragraph::new(lines)
            .block(theme.panel(format!(" {title} "), true))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn group_label(name: &str) -> String {
    match name {
        "browser-firefox" => "Firefox".to_owned(),
        "browser-brave" => "Brave".to_owned(),
        "editor-neovim" => "Neovim / LazyVim".to_owned(),
        "editor-gnome" => "GNOME Text Editor".to_owned(),
        "editor-kate" => "Kate".to_owned(),
        "editor-code" => "VS Code".to_owned(),
        "hype-flow" => "Hype Flow".to_owned(),
        "login-manager-ly" => "Ly display manager".to_owned(),
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

struct Extras<'a> {
    groups: &'a [(String, Vec<String>)],
    checked: &'a [bool],
    cursor: usize,
}

impl Extras<'_> {
    fn draw(self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let Self {
            groups,
            checked,
            cursor,
        } = self;
        let list_height = (area.height / 2).max(4);
        let [list, details] = columns(area, list_height, 34);
        let count = checked.iter().filter(|&&check| check).count();
        let items = groups.iter().zip(checked).map(|((name, _), &check)| {
            let check = if check { "[x]" } else { "[ ]" };
            ListItem::new(format!("{check} {}", group_label(name)))
        });
        let mut state = ListState::default().with_selected(Some(cursor));
        frame.render_stateful_widget(
            List::new(items)
                .block(theme.panel(format!(" Extras {count}/{} ", groups.len()), true))
                .highlight_style(theme.selection())
                .highlight_symbol("› "),
            list,
            &mut state,
        );

        let (name, packages) = &groups[cursor];
        let state = if checked[cursor] {
            "Selected"
        } else {
            "Available"
        };
        let package_label = if packages.len() == 1 {
            "package"
        } else {
            "packages"
        };
        let mut lines = vec![
            Line::from(format!("{state}  /  {} {package_label}", packages.len()))
                .style(theme.fg(LILAC).bold()),
            Line::from(""),
        ];
        lines.extend(
            packages
                .chunks(if details.width >= 55 { 3 } else { 2 })
                .map(|row| Line::from(row.join("  "))),
        );
        lines.push(Line::from(""));
        lines.push(Line::from("Core desktop packages are always included.").style(theme.fg(MUTED)));
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme.panel(format!(" {} ", group_label(name)), false))
                .wrap(Wrap { trim: true }),
            details,
        );
    }
}

struct Progress<'a> {
    phase: Option<usize>,
    steps: &'static [(&'static str, Phase)],
    logs: &'a std::collections::VecDeque<(String, bool)>,
    scroll: usize,
    title: &'a str,
    message: &'a str,
    prompt: Prompt<'a>,
}

impl Progress<'_> {
    fn draw(self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let failed = matches!(self.prompt, Prompt::Finished(true));
        let [sidebar, detail] = columns(area, 3, 32);
        let panel = theme.panel(" Progress ", false);
        let inner = panel.inner(sidebar);
        frame.render_widget(panel, sidebar);
        let completed = self.phase.unwrap_or(0);
        let [meter, phases] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        frame.render_widget(
            Gauge::default()
                .ratio(completed as f64 / self.steps.len() as f64)
                .label(format!(" {completed:02} / {} ", self.steps.len()))
                .gauge_style(theme.fg(if failed { AMBER } else { MINT }).bold()),
            meter,
        );
        if phases.height > 0
            && let Some(current) = self.phase
        {
            let start = current.saturating_sub(phases.height.saturating_sub(1) as usize);
            let items = self
                .steps
                .iter()
                .enumerate()
                .skip(start)
                .map(|(index, (name, _))| {
                    let (mark, color) = if index < current {
                        ("✓", MINT)
                    } else if index == current {
                        (
                            if failed { "!" } else { "›" },
                            if failed { AMBER } else { LILAC },
                        )
                    } else {
                        ("·", MUTED)
                    };
                    ListItem::new(Line::styled(format!("{mark} {name}"), theme.fg(color)))
                });
            frame.render_widget(List::new(items), phases);
        }
        let placeholder = self.message;
        let (title, status, action) = match self.prompt {
            Prompt::Working => (self.title, placeholder, None),
            Prompt::Finished(_) => (self.title, placeholder, None),
            Prompt::Confirm(question, yes, seconds) => {
                let select = |text, active| {
                    Span::styled(
                        text,
                        if active {
                            theme.selection()
                        } else {
                            theme.fg(MUTED)
                        },
                    )
                };
                (
                    " Confirm ",
                    question,
                    Some(Line::from(vec![
                        select(" No ".to_owned(), !yes),
                        Span::raw("  "),
                        select(
                            seconds.map_or_else(
                                || " Yes ".to_owned(),
                                |seconds| format!(" Yes in {seconds}s "),
                            ),
                            yes,
                        ),
                    ])),
                )
            }
            Prompt::Input(question, answer) => (
                " Input ",
                question,
                Some(Line::styled(format!("› {answer}▏"), theme.fg(LILAC))),
            ),
        };
        let width = detail.width.saturating_sub(8).max(1) as usize;
        let status_lines: usize = status
            .lines()
            .map(|line| Line::from(line).width().div_ceil(width).max(1))
            .sum();
        let status_height =
            (status_lines + 2 + usize::from(action.is_some())).min(detail.height as usize) as u16;
        let [status_area, log_area] =
            Layout::vertical([Constraint::Length(status_height), Constraint::Min(0)]).areas(detail);
        let mut lines = status
            .lines()
            .map(|line| Line::styled(line, theme.fg(if failed { AMBER } else { INK })))
            .collect::<Vec<_>>();
        if let Some(action) = action {
            lines.push(action);
        }
        let interactive = matches!(self.prompt, Prompt::Confirm(..) | Prompt::Input(..));
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme.panel(format!(" {} ", title.trim()), interactive))
                .wrap(Wrap { trim: true }),
            status_area,
        );

        let panel = theme.panel(
            if self.scroll > 0 {
                " Activity / scroll "
            } else {
                " Activity "
            },
            false,
        );
        let rows = panel.inner(log_area).height as usize;
        let end = self.logs.len().saturating_sub(self.scroll);
        let start = end.saturating_sub(rows);
        let log_lines = self
            .logs
            .range(start..end)
            .map(|(line, warning)| {
                Line::styled(
                    line.as_str(),
                    theme.fg(if *warning { AMBER } else { MUTED }),
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(log_lines).block(panel), log_area);
    }
}
