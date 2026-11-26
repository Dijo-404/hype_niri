use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use hype_install::{
    CommandOutput, Installer, ProcessRunner, Result, Runner,
    core::{
        CONFIRM_TIMEOUT, FLOW_PHASES, InstallLock, PACKAGE_PHASES, PHASES, Phase, runtime_directory,
    },
    error,
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    fs::{File, OpenOptions},
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod view;

const POLL: Duration = Duration::from_millis(100);
const SUDO_REFRESH: Duration = Duration::from_secs(60);
const PROMPT: &str = "[hype-niri password]";
const MAX_PASSWORD: usize = 256;

#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    Install,
    Update,
    Packages,
    Flow,
}

impl Action {
    pub fn uses_managed_source(self, local_source: bool, fetched: bool) -> bool {
        matches!(self, Self::Install | Self::Update)
            && (fetched || !local_source || self == Self::Update)
    }

    fn phases(self) -> &'static [(&'static str, Phase)] {
        match self {
            Self::Packages => PACKAGE_PHASES,
            Self::Flow => FLOW_PHASES,
            Self::Install | Self::Update => PHASES,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Install => "Install Hype Niri",
            Self::Update => "Update packages and Hype Niri",
            Self::Packages => "Update packages only",
            Self::Flow => "Update Hype Flow only",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Update => "Update",
            Self::Packages => "Package update",
            Self::Flow => "Hype Flow update",
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
        &[Action::Packages, Action::Update, Action::Flow]
    } else {
        &[
            Action::Install,
            Action::Packages,
            Action::Update,
            Action::Flow,
        ]
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
        if matches!(action, Action::Packages | Action::Flow) {
            if ui.borrow_mut().review(None)? {
                break;
            }
            continue;
        }
        if action.uses_managed_source(local_source, fetched) {
            ui.borrow_mut().source = "GitHub, saved to ~/hype-niri/source";
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
            ui.borrow_mut().source = "This folder";
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
        Ok(_) => {
            let ready = ui
                .borrow_mut()
                .working(action.label(), "Checking sudo access");
            ready
                .and_then(|()| {
                    if action == Action::Flow {
                        Ok(())
                    } else {
                        TuiRunner(ui.clone()).authorize()
                    }
                })
                .and_then(|()| {
                    installer.run_phases(action.phases(), |index, name| {
                        let mut ui = ui.borrow_mut();
                        ui.phase = Some(index);
                        ui.working(action.label(), name)
                    })
                })
                .and_then(|()| {
                    if action == Action::Update {
                        ui.borrow_mut()
                            .working(action.label(), "Refreshing the Hype CLI")?;
                        installer.refresh_cli()?;
                    }
                    Ok(())
                })
        }
        Err(cause) => Err(error(cause.to_string())),
    };
    ui.borrow_mut().keep_alive = None;
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

    let mut summary = String::from(match action {
        Action::Packages => "Restart when ready if system components were upgraded.",
        Action::Flow => "Log out and back in if Hype Flow was already running.",
        Action::Install | Action::Update => "Reboot when ready, then select niri-session.",
    });
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
        Ok(self.press(wait)?.map(|(code, _)| code))
    }

    fn press(&self, wait: bool) -> Result<Option<(KeyCode, KeyModifiers)>> {
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
                Ok(Some((code, modifiers)))
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
    started: Instant,
    step_started: Instant,
    activity: String,
    replace_last: bool,
    keep_alive: Option<KeepAlive>,
}

impl Ui {
    fn new() -> Result<Self> {
        Ok(Self {
            screen: Screen::new()?,
            action: Action::Install,
            source: "This folder",
            phase: None,
            logs: VecDeque::new(),
            scroll: 0,
            title: String::new(),
            message: String::new(),
            color: std::env::var_os("NO_COLOR").is_none(),
            started: Instant::now(),
            step_started: Instant::now(),
            activity: String::new(),
            replace_last: false,
            keep_alive: None,
        })
    }

    fn push(&mut self, text: String, warning: bool) {
        if self.logs.len() == 512 {
            self.logs.pop_front();
        }
        self.logs.push_back((text, warning));
        if self.scroll > 0 {
            self.scroll = (self.scroll + 1).min(self.logs.len() - 1);
        }
    }

    fn log(&mut self, message: &str, warning: bool) {
        self.replace_last = false;
        for line in message.lines() {
            self.push(clean(line), warning);
        }
    }

    fn output(&mut self, line: &[u8], carriage: bool) {
        let text = clean(&String::from_utf8_lossy(line));
        if text.trim().is_empty() {
            return;
        }
        if std::mem::replace(&mut self.replace_last, carriage) && self.logs.pop_back().is_some() {
            self.logs.push_back((text, false));
        } else {
            self.push(text, false);
        }
    }

    fn keep_sudo_alive(&mut self) {
        self.keep_alive.get_or_insert_with(KeepAlive::start);
    }

    fn working(&mut self, title: &str, message: &str) -> Result<()> {
        self.screen.check_interrupt()?;
        self.title = title.to_owned();
        self.message = message.to_owned();
        self.step_started = Instant::now();
        self.log(message, false);
        self.scroll = 0;
        self.draw(view::Page::Progress(view::Prompt::Working))
    }

    fn password(&mut self, problem: Option<&str>) -> Result<String> {
        let mut answer = String::with_capacity(MAX_PASSWORD);
        self.scroll = 0;
        let result = loop {
            let page =
                view::Page::Progress(view::Prompt::Password(problem, answer.chars().count()));
            if let Err(cause) = self.draw(page) {
                break Err(cause);
            }
            match self.screen.press(true) {
                Ok(Some((KeyCode::Enter, _))) if !answer.is_empty() => return Ok(answer),
                Ok(Some((KeyCode::Esc, _))) => {
                    break Err(error("Installation stopped without a sudo password"));
                }
                Ok(Some((KeyCode::Backspace, _))) => {
                    answer.pop();
                }
                Ok(Some((KeyCode::Char('u'), modifiers)))
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    answer.clear();
                }
                Ok(Some((KeyCode::Char(c), modifiers)))
                    if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && answer.len() + c.len_utf8() <= MAX_PASSWORD =>
                {
                    answer.push(c);
                }
                Ok(_) => {}
                Err(cause) => break Err(cause),
            }
        };
        wipe(&mut answer);
        result
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

    fn confirm(&mut self, question: &str, explicit: bool) -> Result<bool> {
        let mut yes = !explicit;
        let mut deadline = (!explicit).then(|| Instant::now() + CONFIRM_TIMEOUT);
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
                    if self.confirm("Restart the system now?", false)? {
                        return Ok(true);
                    }
                }
                Some(key) => self.navigate_log(key),
                None => {}
            }
        }
    }
}

struct KeepAlive {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeepAlive {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut waited = Duration::ZERO;
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(POLL);
                waited += POLL;
                if waited >= SUDO_REFRESH {
                    waited = Duration::ZERO;
                    let _ = Command::new("sudo")
                        .args(["-n", "-v"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wipe(text: &mut String) {
    let bytes = unsafe { text.as_mut_vec() };
    bytes.clear();
    bytes.resize(bytes.capacity(), 0);
    std::hint::black_box(bytes);
}

fn command(program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn start(command: &mut Command) -> io::Result<Child> {
    let mut child = command.spawn()?;
    drop(child.stdin.take());
    Ok(child)
}

fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("sudo did not accept the password")
        .to_owned()
}

struct TuiRunner(Rc<RefCell<Ui>>);

impl TuiRunner {
    fn authorize(&self) -> Result<()> {
        if self.captured("sudo", &["-n", "-v"])?.success() {
            self.0.borrow_mut().keep_sudo_alive();
            return Ok(());
        }
        let errors = tempfile::NamedTempFile::new()?;
        let mut reader = errors.reopen()?;
        let mut child = command("sudo", &["-S", "-p", PROMPT, "-v"])
            .stdout(Stdio::null())
            .stderr(errors.as_file().try_clone()?)
            .spawn()?;
        self.0.borrow_mut().activity = "Checking the password".into();
        let result = self.answer(&mut child, &mut reader);
        self.0.borrow_mut().activity.clear();
        if result.is_err() && child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result?;
        if !self
            .captured("/bin/sh", &["-c", "sudo -n -v; exit $?"])?
            .success()
        {
            return Err(error(
                "sudo keeps no reusable login (timestamp_timeout=0 or timestamp_type=ppid); run hype install --plain",
            ));
        }
        let mut ui = self.0.borrow_mut();
        ui.log("Password accepted", false);
        ui.keep_sudo_alive();
        Ok(())
    }

    fn answer(&self, child: &mut Child, reader: &mut File) -> Result<()> {
        let mut input = child.stdin.take();
        let mut bytes = Vec::new();
        let mut asked = 0;
        loop {
            reader.read_to_end(&mut bytes)?;
            let text = String::from_utf8_lossy(&bytes);
            let prompts = text.matches(PROMPT).count();
            if prompts > asked {
                asked = prompts;
                let before = text[..text.rfind(PROMPT).unwrap_or(0)].replace(PROMPT, "");
                let problem = (asked > 1).then(|| last_line(&before));
                let mut password = self.0.borrow_mut().password(problem.as_deref())?;
                if let Some(pipe) = input.as_mut() {
                    let _ = pipe
                        .write_all(password.as_bytes())
                        .and(pipe.write_all(b"\n"));
                }
                wipe(&mut password);
                continue;
            }
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(error(format!(
                        "sudo did not accept the password: {}",
                        last_line(&text.replace(PROMPT, ""))
                    )))
                };
            }
            self.0
                .borrow_mut()
                .draw(view::Page::Progress(view::Prompt::Working))?;
            self.0.borrow().screen.key(false)?;
        }
    }

    fn privileged<'a>(&self, program: &str, args: &[&'a str]) -> Result<Vec<&'a str>> {
        let mut args = args.to_vec();
        match program {
            "sudo" => {
                self.authorize()?;
                args.insert(0, "-n");
            }
            "yay" => {
                self.authorize()?;
                args.splice(1.min(args.len())..1.min(args.len()), ["--sudoflags", "-n"]);
            }
            "env" => self.authorize()?,
            _ => {}
        }
        Ok(args)
    }

    fn wait(
        &self,
        child: &mut Child,
        mut read: impl FnMut(&mut Ui, bool) -> io::Result<()>,
    ) -> Result<ExitStatus> {
        let mut stopping = false;
        let result = (|| {
            loop {
                let status = child.try_wait()?;
                {
                    let mut ui = self.0.borrow_mut();
                    read(&mut ui, status.is_some())?;
                    ui.draw(view::Page::Progress(view::Prompt::Working))?;
                }
                if let Some(status) = status {
                    return if stopping {
                        Err(error("Installation interrupted"))
                    } else {
                        Ok(status)
                    };
                }
                if stopping {
                    std::thread::sleep(POLL);
                    continue;
                }
                let key = self.0.borrow().screen.key(false);
                match key {
                    Ok(Some(key)) => self.0.borrow_mut().navigate_log(key),
                    Ok(None) => {}
                    Err(_) => {
                        unsafe { libc::kill(0, libc::SIGINT) };
                        self.0
                            .borrow_mut()
                            .log("Stopping after the current command exits", true);
                        stopping = true;
                    }
                }
            }
        })();
        if result.is_err() && child.try_wait().ok().flatten().is_none() {
            if !stopping {
                unsafe { libc::kill(0, libc::SIGINT) };
            }
            let _ = child.wait();
        }
        result
    }

    fn captured(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        let mut child = start(
            command(program, args)
                .stdout(stdout.try_clone()?)
                .stderr(stderr.try_clone()?),
        )?;
        self.0.borrow_mut().activity = command_line(program, args);
        let status = self.wait(&mut child, |_, _| Ok(()));
        self.0.borrow_mut().activity.clear();
        let status = status?;
        let text = |file: &mut File| -> io::Result<String> {
            let mut bytes = Vec::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_end(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        };
        Ok(CommandOutput {
            code: status.code().unwrap_or(1),
            stdout: text(&mut stdout)?,
            stderr: text(&mut stderr)?,
        })
    }

    fn streamed(&self, program: &str, args: &[&str], log: Option<&Path>) -> Result<()> {
        let (sink, mut reader) = match log {
            Some(path) => {
                let sink = OpenOptions::new().create(true).append(true).open(path)?;
                let mut reader = File::open(path)?;
                reader.seek(SeekFrom::End(0))?;
                (sink, reader)
            }
            None => {
                let file = tempfile::NamedTempFile::new()?;
                (file.as_file().try_clone()?, file.reopen()?)
            }
        };
        let mut child = start(
            command(program, args)
                .stdout(sink.try_clone()?)
                .stderr(sink),
        )?;
        self.0.borrow_mut().activity = command_line(program, args);
        let mut pending = Vec::new();
        let status = self.wait(&mut child, |ui, finished| {
            drain_log(&mut reader, &mut pending, finished, ui)
        });
        self.0.borrow_mut().activity.clear();
        let status = status?;
        if status.success() {
            Ok(())
        } else {
            Err(error(format!("{program} failed ({status})")))
        }
    }
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

fn drain_log(
    reader: &mut File,
    pending: &mut Vec<u8>,
    finished: bool,
    ui: &mut Ui,
) -> io::Result<()> {
    reader.read_to_end(pending)?;
    let mut start = 0;
    let mut index = 0;
    while index < pending.len() {
        match pending[index] {
            b'\n' => {
                ui.output(&pending[start..index], false);
                start = index + 1;
            }
            b'\r' => match pending.get(index + 1) {
                Some(b'\n') => {
                    ui.output(&pending[start..index], false);
                    index += 1;
                    start = index + 1;
                }
                Some(_) => {
                    ui.output(&pending[start..index], true);
                    start = index + 1;
                }
                None if !finished => break,
                None => {
                    ui.output(&pending[start..index], false);
                    start = index + 1;
                }
            },
            _ => {}
        }
        index += 1;
    }
    if finished || pending.len() - start >= 16 * 1024 {
        ui.output(&pending[start..], false);
        start = pending.len();
    }
    pending.drain(..start);
    Ok(())
}

fn clean(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => text.push(' '),
            c if !c.is_control() => text.push(c),
            _ => {}
        }
    }
    text
}

impl Runner for TuiRunner {
    fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        self.0.borrow().screen.check_interrupt()?;
        if program == "sudo" {
            self.captured(program, &self.privileged(program, args)?)
        } else {
            self.captured(program, args)
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        self.0.borrow().screen.check_interrupt()?;
        self.streamed(program, &self.privileged(program, args)?, None)
    }

    fn run_logged(&self, program: &str, args: &[&str], log: &Path) -> Result<()> {
        self.0.borrow().screen.check_interrupt()?;
        self.streamed(program, &self.privileged(program, args)?, Some(log))
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        self.0.borrow_mut().confirm(prompt, false)
    }

    fn confirm_explicit(&self, prompt: &str) -> Result<bool> {
        self.0.borrow_mut().confirm(prompt, true)
    }

    fn input(&self, prompt: &str) -> Result<String> {
        self.0.borrow_mut().input(prompt)
    }

    fn available(&self, program: &str) -> bool {
        ProcessRunner.available(program)
    }

    fn unattended(&self) -> bool {
        true
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
