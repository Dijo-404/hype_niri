use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use hype_install::{
    CommandOutput, Installer, ProcessRunner, Result, Runner,
    core::{CONFIRM_TIMEOUT, InstallLock, PACKAGE_PHASES, PHASES, Phase, runtime_directory},
    error,
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::Path,
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod view;

const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    Install,
    Update,
    Packages,
}

impl Action {
    pub fn uses_managed_source(self, local_source: bool, fetched: bool) -> bool {
        self != Self::Packages && (fetched || !local_source || self == Self::Update)
    }

    fn phases(self) -> &'static [(&'static str, Phase)] {
        if self == Self::Packages {
            PACKAGE_PHASES
        } else {
            PHASES
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Install => "Install desktop",
            Self::Update => "Update packages and Hype Niri",
            Self::Packages => "Update system packages",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Update => "Update",
            Self::Packages => "Package update",
        }
    }
}

pub fn available() -> bool {
    let term = std::env::var_os("TERM");
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && term.is_some_and(|term| !term.is_empty() && term != "dumb")
}

pub fn update_defaults(
    installer: &Installer,
    groups: &[(String, Vec<String>)],
) -> Result<Vec<String>> {
    let saved = installer.saved_optional_groups()?.unwrap_or_default();
    Ok(groups
        .iter()
        .filter(|(name, _)| saved.contains(name))
        .map(|(name, _)| name.clone())
        .collect())
}

pub fn run(
    mut installer: Installer,
    direct: Option<Action>,
    local_source: bool,
    fetched: bool,
) -> Result<()> {
    let original = installer.source.clone();
    let installed = installer.user(".config/scripts/hype-control").is_file()
        || installer.user(".config/scripts/hype-control.sh").is_file();
    let choices: &[Action] = if direct == Some(Action::Update) {
        &[Action::Packages, Action::Update]
    } else {
        &[Action::Install, Action::Packages, Action::Update]
    };
    let mut action = if direct == Some(Action::Install) {
        Action::Install
    } else if installed || direct == Some(Action::Update) {
        Action::Packages
    } else {
        Action::Install
    };
    let ui = Rc::new(RefCell::new(Ui::new()?));
    installer.runner = Box::new(TuiRunner(ui.clone()));

    let mut managed_source = None;
    let mut direct = direct == Some(Action::Install);
    loop {
        if !direct {
            let Some(choice) = ui.borrow_mut().choose_action(action, choices)? else {
                return Ok(());
            };
            action = choice;
        }
        direct = false;
        ui.borrow_mut().action = action;
        if action == Action::Packages {
            if ui.borrow_mut().review(None)? {
                break;
            }
            continue;
        }
        if action.uses_managed_source(local_source, fetched) {
            ui.borrow_mut().source = "~/hype-niri/source (GitHub main)";
            if managed_source.is_none() {
                ui.borrow_mut()
                    .working("Preparing desktop source", "Checking ~/hype-niri/source")?;
                match installer.prepare_managed_source(!fetched) {
                    Ok(lock) => managed_source = Some((installer.source.clone(), lock)),
                    Err(cause) => {
                        ui.borrow_mut()
                            .finish("Source setup failed", &cause.to_string(), true)?;
                        return Err(cause);
                    }
                }
            }
            installer
                .source
                .clone_from(&managed_source.as_ref().unwrap().0);
        } else {
            ui.borrow_mut().source = "Local checkout";
            installer.source.clone_from(&original);
        }

        let groups = installer.optional_package_groups()?;
        let saved = if action == Action::Update {
            update_defaults(&installer, &groups)?
        } else {
            Vec::new()
        };
        let checked = groups
            .iter()
            .map(|(name, _)| action == Action::Install || saved.contains(name))
            .collect();
        if let Some(selection) = ui.borrow_mut().choose_groups(&groups, checked)? {
            installer.optional_groups = Some(selection);
            break;
        }
    }

    let lock = runtime_directory().and_then(|directory| InstallLock::acquire(&directory));
    let result = match &lock {
        Ok(_) => installer
            .run_phases(action.phases(), |index, name| {
                let mut ui = ui.borrow_mut();
                ui.phase = Some(index);
                ui.working(action.label(), name)
            })
            .and_then(|()| {
                if action == Action::Update {
                    ui.borrow_mut()
                        .working(action.label(), "Refreshing the Hype CLI")?;
                    installer.refresh_cli()?;
                }
                Ok(())
            }),
        Err(cause) => Err(error(cause.to_string())),
    };
    if let Err(cause) = result {
        let mut ui = ui.borrow_mut();
        if let Some(backup) = &installer.backup {
            ui.log(
                &format!("Configuration backup: {}", backup.display()),
                false,
            );
        }
        ui.finish(
            &format!("{} failed", action.title()),
            &cause.to_string(),
            true,
        )?;
        ui.screen.leave()?;
        if let Some(backup) = &installer.backup {
            eprintln!("Configuration backup: {}", backup.display());
        }
        return Err(cause);
    }

    let mut summary = if action == Action::Packages {
        String::from("Restart when ready if system components were upgraded.")
    } else {
        String::from("Reboot when ready, then select niri-session.")
    };
    if let Some(backup) = &installer.backup {
        summary.push_str(&format!("\nConfiguration backup: {}", backup.display()));
    }
    if installer.selected("hype-flow") {
        summary.push_str("\nAlt+Space starts or finishes dictation.");
    }
    let reboot = {
        let mut ui = ui.borrow_mut();
        ui.phase = Some(action.phases().len());
        ui.finish(&format!("{} complete", action.title()), &summary, false)?
    };
    ui.borrow_mut().screen.leave()?;
    println!("{} complete. {summary}", action.title());
    if reboot {
        ProcessRunner.run("systemctl", &["reboot"])?;
    }
    Ok(())
}

struct TerminalOutput(Option<io::Stdout>);

impl TerminalOutput {
    fn send<T>(
        &mut self,
        fallback: T,
        write: impl FnOnce(&mut io::Stdout) -> io::Result<T>,
    ) -> io::Result<T> {
        let Some(output) = &mut self.0 else {
            return Ok(fallback);
        };
        let result = write(output);
        if result
            .as_ref()
            .is_err_and(|cause| matches!(cause.raw_os_error(), Some(libc::EIO | libc::EPIPE)))
        {
            // Report the disconnect once. Ratatui may still write during cursor cleanup.
            self.0 = None;
        }
        result
    }
}

impl Write for TerminalOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.send(bytes.len(), |output| output.write(bytes))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send((), Write::flush)
    }
}

type TerminalBackend = Terminal<CrosstermBackend<TerminalOutput>>;

struct Screen {
    terminal: TerminalBackend,
    active: bool,
    interrupted: Arc<AtomicBool>,
    signals: Vec<signal_hook::SigId>,
}

impl Screen {
    fn new() -> Result<Self> {
        let mut screen = Self {
            terminal: Terminal::new(CrosstermBackend::new(TerminalOutput(Some(io::stdout()))))?,
            active: false,
            interrupted: Arc::new(AtomicBool::new(false)),
            signals: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            screen.signals.push(signal_hook::flag::register(
                signal,
                screen.interrupted.clone(),
            )?);
        }
        screen.enter()?;
        Ok(screen)
    }

    fn enter(&mut self) -> io::Result<()> {
        self.active = true;
        terminal::enable_raw_mode()?;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        self.terminal.hide_cursor()?;
        let (width, height) = terminal::size()?;
        self.terminal.resize(Rect::new(0, 0, width, height))
    }

    fn leave(&mut self) -> io::Result<()> {
        if self.active {
            self.active = false;
            let raw = terminal::disable_raw_mode();
            let cursor = self.terminal.show_cursor();
            let alternate = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            raw.and(cursor).and(alternate)?;
        }
        Ok(())
    }

    fn check_interrupt(&self) -> Result<()> {
        if self.interrupted.load(Ordering::Relaxed) {
            return Err(error("Installation interrupted"));
        }
        let mut input = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: 0,
            revents: 0,
        };
        if unsafe { libc::poll(&mut input, 1, 0) } > 0
            && input.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        {
            return Err(error("Installer terminal disconnected"));
        }
        Ok(())
    }

    fn confirmation_seconds(&self, deadline: &mut Instant) -> Result<u64> {
        self.check_interrupt()?;
        let (width, height) = terminal::size()?;
        let now = Instant::now();
        if width < view::MIN_WIDTH || height < view::MIN_HEIGHT {
            *deadline = now + CONFIRM_TIMEOUT;
        }
        Ok(deadline
            .saturating_duration_since(now)
            .as_millis()
            .div_ceil(1000) as u64)
    }

    fn key(&self, wait: bool) -> Result<Option<KeyCode>> {
        let event = loop {
            self.check_interrupt()?;
            if event::poll(POLL)? {
                break event::read()?;
            }
            if !wait {
                return Ok(None);
            }
        };
        match event {
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }) => {
                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                    self.interrupted.store(true, Ordering::Relaxed);
                    self.check_interrupt()?;
                }
                let (width, height) = terminal::size()?;
                if (width < view::MIN_WIDTH || height < view::MIN_HEIGHT)
                    && !matches!(code, KeyCode::Esc | KeyCode::Char('q'))
                {
                    return Ok(None);
                }
                Ok(Some(code))
            }
            _ => Ok(None),
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.leave();
        for signal in self.signals.drain(..) {
            signal_hook::low_level::unregister(signal);
        }
    }
}

struct Ui {
    screen: Screen,
    action: Action,
    source: &'static str,
    phase: Option<usize>,
    logs: VecDeque<(String, bool)>,
    scroll: usize,
    title: String,
    message: String,
    color: bool,
}

impl Ui {
    fn new() -> Result<Self> {
        Ok(Self {
            screen: Screen::new()?,
            action: Action::Install,
            source: "Local checkout",
            phase: None,
            logs: VecDeque::new(),
            scroll: 0,
            title: String::new(),
            message: String::new(),
            color: std::env::var_os("NO_COLOR").is_none(),
        })
    }

    fn log(&mut self, message: &str, warning: bool) {
        for line in message.lines() {
            if self.logs.len() == 512 {
                self.logs.pop_front();
            }
            let text = line
                .chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .filter(|c| !c.is_control())
                .collect();
            self.logs.push_back((text, warning));
            if self.scroll > 0 {
                self.scroll = (self.scroll + 1).min(self.logs.len() - 1);
            }
        }
    }

    fn working(&mut self, title: &str, message: &str) -> Result<()> {
        self.screen.check_interrupt()?;
        self.title = title.to_owned();
        self.message = message.to_owned();
        self.log(message, false);
        self.scroll = 0;
        self.draw(view::Page::Progress(view::Prompt::Working))
    }

    fn choose_action(&mut self, action: Action, choices: &[Action]) -> Result<Option<Action>> {
        let mut selected = choices
            .iter()
            .position(|choice| *choice == action)
            .unwrap_or(0);
        loop {
            self.draw(view::Page::Actions { choices, selected })?;
            match self.screen.key(true)? {
                Some(KeyCode::Tab | KeyCode::Down | KeyCode::Char('j')) => {
                    selected = (selected + 1) % choices.len();
                }
                Some(KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k')) => {
                    selected = (selected + choices.len() - 1) % choices.len();
                }
                Some(KeyCode::Enter) => return Ok(Some(choices[selected])),
                Some(KeyCode::Esc | KeyCode::Char('q')) => return Ok(None),
                _ => {}
            }
        }
    }

    fn choose_groups(
        &mut self,
        groups: &[(String, Vec<String>)],
        mut checked: Vec<bool>,
    ) -> Result<Option<Vec<String>>> {
        let mut cursor = 0;
        loop {
            self.draw(view::Page::Extras {
                groups,
                checked: &checked,
                cursor,
            })?;
            match self.screen.key(true)? {
                Some(KeyCode::Enter | KeyCode::Tab) => {
                    let selected = groups
                        .iter()
                        .zip(&checked)
                        .filter(|(_, check)| **check)
                        .map(|((name, _), _)| name.clone())
                        .collect::<Vec<_>>();
                    if self.review(Some(&selected))? {
                        return Ok(Some(selected));
                    }
                }
                Some(KeyCode::Esc | KeyCode::Char('q')) => return Ok(None),
                Some(KeyCode::Up | KeyCode::Char('k')) => cursor = cursor.saturating_sub(1),
                Some(KeyCode::Down | KeyCode::Char('j')) => {
                    cursor = (cursor + 1).min(groups.len() - 1)
                }
                Some(KeyCode::Char(' ')) => checked[cursor] = !checked[cursor],
                Some(KeyCode::Char('a')) => checked.fill(true),
                Some(KeyCode::Char('n')) => checked.fill(false),
                _ => {}
            }
        }
    }

    fn review(&mut self, extras: Option<&[String]>) -> Result<bool> {
        let mut deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            let seconds = self.screen.confirmation_seconds(&mut deadline)?;
            if seconds == 0 {
                return Ok(true);
            }
            self.draw(view::Page::Review { extras, seconds })?;
            match self.screen.key(false)? {
                Some(KeyCode::Enter | KeyCode::Char('y' | 'Y')) => return Ok(true),
                Some(KeyCode::Esc | KeyCode::BackTab | KeyCode::Char('q' | 'n' | 'N')) => {
                    return Ok(false);
                }
                _ => {}
            }
        }
    }

    fn navigate_log(&mut self, key: KeyCode) {
        self.scroll = match key {
            KeyCode::PageUp => (self.scroll + 5).min(self.logs.len().saturating_sub(1)),
            KeyCode::PageDown => self.scroll.saturating_sub(5),
            KeyCode::End => 0,
            _ => self.scroll,
        };
    }

    fn confirm(&mut self, question: &str) -> Result<bool> {
        let mut yes = true;
        let mut deadline = Some(Instant::now() + CONFIRM_TIMEOUT);
        self.scroll = 0;
        loop {
            let seconds = deadline
                .as_mut()
                .map(|deadline| self.screen.confirmation_seconds(deadline))
                .transpose()?;
            if seconds == Some(0) {
                return Ok(true);
            }
            self.draw(view::Page::Progress(view::Prompt::Confirm(
                question, yes, seconds,
            )))?;
            match self.screen.key(false)? {
                Some(KeyCode::Enter) => return Ok(yes),
                Some(KeyCode::Char('y' | 'Y')) => return Ok(true),
                Some(KeyCode::Esc | KeyCode::Char('n' | 'N')) => return Ok(false),
                Some(KeyCode::Left | KeyCode::Right | KeyCode::Tab) => {
                    yes = !yes;
                    deadline = None;
                }
                Some(key) => self.navigate_log(key),
                None => {}
            }
        }
    }

    fn input(&mut self, question: &str) -> Result<String> {
        let mut answer = String::new();
        self.scroll = 0;
        loop {
            self.draw(view::Page::Progress(view::Prompt::Input(question, &answer)))?;
            match self.screen.key(true)? {
                Some(KeyCode::Enter) => return Ok(answer),
                Some(KeyCode::Esc) => return Err(error("Installation interrupted")),
                Some(KeyCode::Char(c)) if answer.len() < 128 => answer.push(c),
                Some(KeyCode::Backspace) => {
                    answer.pop();
                }
                Some(key) => self.navigate_log(key),
                None => {}
            }
        }
    }

    fn finish(&mut self, title: &str, message: &str, failed: bool) -> Result<bool> {
        self.title = title.to_owned();
        self.message = message.to_owned();
        self.log(message, failed);
        self.scroll = 0;
        loop {
            self.draw(view::Page::Progress(view::Prompt::Finished(failed)))?;
            match self.screen.key(true)? {
                Some(KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) => return Ok(false),
                Some(KeyCode::Char('r')) if !failed => {
                    if self.confirm("Restart the system now?")? {
                        return Ok(true);
                    }
                }
                Some(key) => self.navigate_log(key),
                None => {}
            }
        }
    }
}

struct TuiRunner(Rc<RefCell<Ui>>);

impl TuiRunner {
    fn terminal_command<T>(&self, program: &str, run: impl FnOnce() -> Result<T>) -> Result<T> {
        {
            let mut ui = self.0.borrow_mut();
            ui.screen.check_interrupt()?;
            ui.log(&format!("Terminal: {program}"), false);
            ui.screen.leave()?;
        }
        let result = run();
        let mut ui = self.0.borrow_mut();
        ui.screen.check_interrupt()?;
        ui.screen.enter()?;
        ui.draw(view::Page::Progress(view::Prompt::Working))?;
        result
    }

    fn streamed(&self, program: &str, args: &[&str]) -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let mut reader = file.reopen()?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(file.as_file().try_clone()?)
            .stderr(file.as_file().try_clone()?)
            .spawn()?;
        let mut pending = Vec::new();
        let result = (|| {
            loop {
                let status = child.try_wait()?;
                drain_log(
                    &mut reader,
                    &mut pending,
                    status.is_some(),
                    &mut self.0.borrow_mut(),
                )?;
                self.0
                    .borrow_mut()
                    .draw(view::Page::Progress(view::Prompt::Working))?;
                if let Some(status) = status {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(error(format!("{program} failed ({status})")))
                    };
                }
                let mut ui = self.0.borrow_mut();
                if let Some(key) = ui.screen.key(false)? {
                    ui.navigate_log(key);
                }
            }
        })();
        if result.is_err() && child.try_wait()?.is_none() {
            let _ = child.kill();
            child.wait()?;
        }
        result
    }
}

fn drain_log(
    reader: &mut File,
    pending: &mut Vec<u8>,
    finished: bool,
    ui: &mut Ui,
) -> io::Result<()> {
    reader.read_to_end(pending)?;
    let end = if finished || pending.len() >= 16 * 1024 {
        pending.len()
    } else {
        pending
            .iter()
            .rposition(|byte| matches!(byte, b'\n' | b'\r'))
            .map_or(0, |offset| offset + 1)
    };
    let lines = String::from_utf8_lossy(&pending[..end]);
    for line in lines
        .split(['\n', '\r'])
        .filter(|line| !line.trim().is_empty())
    {
        ui.log(line, false);
    }
    pending.drain(..end);
    Ok(())
}

impl Runner for TuiRunner {
    fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        self.0.borrow().screen.check_interrupt()?;
        if program == "sudo" {
            self.terminal_command(program, || ProcessRunner.output(program, args))
        } else {
            ProcessRunner.output(program, args)
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        self.0.borrow().screen.check_interrupt()?;
        match program {
            "sudo" | "yay" | "chsh" | "env" => {
                self.terminal_command(program, || ProcessRunner.run(program, args))
            }
            "cargo" | "git" => self.streamed(program, args),
            _ => {
                let output = ProcessRunner.output(program, args)?;
                self.0.borrow_mut().log(&output.stdout, false);
                self.0.borrow_mut().log(&output.stderr, !output.success());
                if output.success() {
                    Ok(())
                } else {
                    Err(error(format!(
                        "{program} failed (exit status: {})",
                        output.code
                    )))
                }
            }
        }
    }

    fn run_logged(&self, program: &str, args: &[&str], log: &Path) -> Result<()> {
        let ui = self.0.borrow();
        let phase = ui
            .phase
            .and_then(|index| ui.action.phases().get(index))
            .map(|entry| entry.0);
        drop(ui);
        self.terminal_command(program, || {
            if let Some(phase) = phase {
                eprintln!("\nHype Niri  /  {phase}");
            }
            ProcessRunner.run_logged(program, args, log)
        })
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        self.0.borrow_mut().confirm(prompt)
    }

    fn input(&self, prompt: &str) -> Result<String> {
        self.0.borrow_mut().input(prompt)
    }

    fn available(&self, program: &str) -> bool {
        ProcessRunner.available(program)
    }

    fn message(&self, message: fmt::Arguments<'_>, warning: bool) {
        self.0.borrow_mut().log(&message.to_string(), warning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_preselects_saved_groups_still_in_the_manifest() {
        let home = tempfile::tempdir().unwrap();
        let mut installer = Installer {
            source: home.path().into(),
            home: home.path().into(),
            root: home.path().into(),
            backup: None,
            runner: Box::new(ProcessRunner),
            optional_groups: None,
            build_dir: None,
        };
        let groups = vec![("firefox".into(), vec![]), ("docker".into(), vec![])];
        assert!(update_defaults(&installer, &groups).unwrap().is_empty());
        installer.optional_groups = Some(vec!["retired".into(), "docker".into()]);
        installer.save_optional_groups().unwrap();
        assert_eq!(update_defaults(&installer, &groups).unwrap(), ["docker"]);
    }
}
