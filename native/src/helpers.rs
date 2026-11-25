use crate::{Result, bus, caffeine, dialog, err, ipc, launcher, runtime, session};
use gtk::{gio, glib, prelude::*};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    fs,
    io::{Read, Seek},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::MetadataExt,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::Path,
    process::{Command, ExitStatus, Stdio},
    ptr, thread,
    time::{Duration, Instant},
};

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const SYSTEMD_UNIT: &str = "org.freedesktop.systemd1.Unit";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const LOGIN: &str = "org.freedesktop.login1";
const LOGIN_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIN_PATH: &str = "/org/freedesktop/login1";
const WAYBAR_WAIT: Duration = Duration::from_secs(3);
const MENU_LOCK: &str = "hype-menu-toggle.lock";
const MENU_WAIT: Duration = Duration::from_secs(1);
const TRAY_WATCHER: &str = "org.kde.StatusNotifierWatcher";
const TRAY_ITEM: &str = "org.kde.StatusNotifierItem";
const TRAY_WAIT: Duration = Duration::from_secs(1);

fn matches_process(pid: i32, name: &str) -> bool {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    let mut named = false;
    let mut live = false;
    let mut owned = false;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("Name:") {
            named = value.trim() == name;
        } else if let Some(value) = line.strip_prefix("State:") {
            live = !value.trim_start().starts_with(['Z', 'X', 'x']);
        } else if let Some(value) = line.strip_prefix("Uid:") {
            owned = value
                .split_whitespace()
                .nth(1)
                .and_then(|uid| uid.parse().ok())
                == Some(runtime::uid());
            break;
        }
    }
    named && live && owned
}

fn named_processes(name: &str) -> impl Iterator<Item = i32> {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(move |pid| matches_process(*pid, name))
}

fn terminate_named(name: &str) -> bool {
    let mut signaled = false;
    for pid in named_processes(name) {
        // Recheck after opening the pidfd so PID reuse cannot signal another process.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            continue;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        if matches_process(pid, name) {
            signaled |= unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGTERM,
                    ptr::null::<libc::siginfo_t>(),
                    0,
                ) == 0
            };
        }
    }
    signaled
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

fn menu_started(pid: u32) -> Option<u64> {
    let proc = Path::new("/proc").join(pid.to_string());
    if proc.metadata().ok()?.uid() != runtime::uid() {
        return None;
    }
    let stat = fs::read_to_string(proc.join("stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn tracked_menu(path: &Path) -> Option<(u32, u64)> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn close_tracked_menu(path: &Path) -> Result<bool> {
    let Some((pid, started)) = tracked_menu(path) else {
        return Ok(false);
    };
    if menu_started(pid) != Some(started) {
        return Ok(false);
    }
    let Some(process) = runtime::Process::open(pid)? else {
        return Ok(false);
    };
    if menu_started(pid) != Some(started) || process.exited()? {
        return Ok(false);
    }
    process.terminate()?;
    let deadline = Instant::now() + MENU_WAIT;
    while !process.exited()? {
        if Instant::now() >= deadline {
            return Err(err("Menu did not exit after its close request"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(true)
}

fn menu_toggle(args: &[OsString]) -> Result<i32> {
    let Some((program, arguments)) = args.split_first() else {
        return Err(err("Usage: menu-toggle PROGRAM [arguments]"));
    };
    let state = runtime::dir()?.join("hype-menu-toggle.state");
    let lock = runtime::lock(MENU_LOCK, MENU_WAIT)?.ok_or_else(|| err("Menu toggle is busy"))?;
    // Preserve the legacy behavior for Wofi launched through a shell wrapper or separately.
    if close_tracked_menu(&state)? || terminate_named("wofi") {
        return Ok(0);
    }
    let mut child = Command::new(program).args(arguments).spawn()?;
    let record = menu_started(child.id())
        .map(|started| (child.id(), started))
        .ok_or_else(|| err("Could not identify the launched menu"));
    let record = match record.and_then(|record| {
        runtime::atomic_write(&state, &serde_json::to_vec(&record)?)?;
        Ok(record)
    }) {
        Ok(record) => record,
        Err(cause) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(cause);
        }
    };
    drop(lock);
    let status = child.wait();
    if let Ok(Some(_lock)) = runtime::lock(MENU_LOCK, MENU_WAIT)
        && tracked_menu(&state) == Some(record)
    {
        let _ = fs::remove_file(state);
    }
    Ok(exit_code(status?))
}

async fn password_prompt(args: &[OsString]) -> Result<i32> {
    let title = match args {
        [] => "Password",
        [flag, title] if flag == "-p" || flag == "--prompt" => title
            .to_str()
            .ok_or_else(|| err("Password prompt must be UTF-8"))?,
        [flag] => flag
            .to_str()
            .and_then(|flag| flag.strip_prefix("--prompt="))
            .ok_or_else(|| err("Usage: password-prompt [-p|--prompt TITLE]"))?,
        _ => return Err(err("Usage: password-prompt [-p|--prompt TITLE]")),
    };
    let Some(password) = dialog::prompt(title, true).await? else {
        return Ok(1);
    };
    println!("{password}");
    Ok(0)
}

async fn logind_power(method: &str) -> Result<()> {
    bus::call(
        &bus::system().await?,
        LOGIN,
        LOGIN_PATH,
        LOGIN_MANAGER,
        method,
        Some(&(true,).to_variant()),
        30_000,
    )
    .await?;
    Ok(())
}

async fn power_menu() -> Result<()> {
    let choices = ["Lock", "Sleep", "Logout", "Shutdown", "Reboot"].map(str::to_owned);
    let Some(choice) =
        dialog::choose_with_keys("Power", &choices, &['l', 's', 'e', 'p', 'r']).await?
    else {
        return Ok(());
    };
    power_action(["lock", "suspend", "logout", "shutdown", "reboot"][choice]).await
}

async fn power_action(action: &str) -> Result<()> {
    match action {
        "lock" => return session::run("lock-screen", &[]).await,
        "suspend" => return session::run("suspend-now", &[]).await,
        "logout" | "shutdown" | "reboot" | "restart" => {
            if let Err(error) = caffeine::run("caffeine", &["stop".into()]).await {
                eprintln!("Could not stop Caffeine: {error}");
            }
        }
        _ => return Err(err("Unknown power action")),
    }
    match action {
        "logout" => ipc::action(json!({"Quit": {"skip_confirmation": true}})),
        "shutdown" => logind_power("PowerOff").await,
        _ => logind_power("Reboot").await,
    }
}

fn app_with_theme(name: &str) -> Command {
    let mut command = Command::new(name);
    let theme = env::var_os("GTK_THEME")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Adwaita:dark".into());
    command.env("GTK_THEME", theme);
    command
}

fn blueman_manager(args: &[OsString]) -> Result<()> {
    let module = env::current_exe()?.with_file_name("libhype_blueman_keys.so");
    let mut command = Command::new("/usr/bin/blueman-manager");
    command.args(args);
    if module.is_file() {
        let mut modules = env::var_os("GTK3_MODULES").unwrap_or_default();
        if !modules.is_empty() {
            modules.push(":");
        }
        modules.push(module);
        command.env("GTK3_MODULES", modules);
    }
    Err(command.exec().into())
}

fn start_tray_applets() -> Result<()> {
    let Some(_launching) = runtime::lock("hype-tray-applets.lock", Duration::ZERO)? else {
        return Ok(());
    };
    let config = env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(runtime::home()?.join(".config"));
    let groups = fs::read_to_string(config.join("hype-niri/optional-groups")).unwrap_or_default();
    for (name, args, group) in [
        ("nm-applet", &["--indicator"][..], None),
        ("blueman-applet", &[][..], Some("bluetooth")),
        ("warp-taskbar", &[][..], Some("warp")),
    ] {
        if group.is_some_and(|group| !groups.lines().any(|line| line.trim() == group)) {
            continue;
        }
        if named_processes(name).next().is_some() {
            continue;
        }
        let Some(lock) = runtime::lock(&format!("hype-tray-{name}.lock"), Duration::ZERO)? else {
            continue;
        };
        let mut command = app_with_theme(name);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        lock.inherit(&mut command);
        // Optional applets may be absent. The lock descriptor follows each launched applet.
        let _ = command.spawn();
    }
    Ok(())
}

fn start_waybar(restart: bool) -> Result<()> {
    let began = Instant::now();
    if restart && terminate_named("waybar") {
        while named_processes("waybar").next().is_some() && began.elapsed() < WAYBAR_WAIT {
            thread::sleep(Duration::from_millis(20));
        }
    }
    let Some(lock) = runtime::lock(
        "hype-waybar.lock",
        WAYBAR_WAIT.saturating_sub(began.elapsed()),
    )?
    else {
        return Ok(());
    };
    if named_processes("waybar").next().is_some() {
        return Ok(());
    }
    let config = match env::var_os("WAYBAR_CONFIG").filter(|path| !path.is_empty()) {
        Some(path) => path.into(),
        None => runtime::home()?.join(".config/waybar/config.jsonc"),
    };
    let mut command = app_with_theme("waybar");
    command.arg("--config").arg(config);
    lock.inherit(&mut command);
    Err(command.exec().into())
}

async fn active_unit(unit: &str) -> Result<bool> {
    let conn = bus::system().await?;
    let path = bus::call(
        &conn,
        SYSTEMD,
        SYSTEMD_PATH,
        SYSTEMD_MANAGER,
        "GetUnit",
        Some(&(unit,).to_variant()),
        2_000,
    )
    .await?
    .get::<(glib::variant::ObjectPath,)>()
    .ok_or_else(|| err("systemd returned an invalid unit path"))?
    .0;
    let state: String = bus::property(&conn, SYSTEMD, &path, SYSTEMD_UNIT, "ActiveState").await?;
    Ok(matches!(
        state.as_str(),
        "active" | "reloading" | "refreshing"
    ))
}

fn bounded_output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    // Redirect to a file so a full pipe or inherited writer cannot extend the deadline.
    let mut output = tempfile::tempfile().ok()?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                return None;
            }
            output.rewind().ok()?;
            let mut bytes = Vec::new();
            output.take(1024 * 1024).read_to_end(&mut bytes).ok()?;
            return Some(bytes);
        }
        if Instant::now() >= deadline {
            // Kill the whole vendor command group, including any helper retaining output.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

async fn stealth_indicator() -> Result<()> {
    let state = Path::new("/run/stealth");
    let active = state.is_dir()
        && active_unit("tor.service").await.unwrap_or(false)
        && (state.join("active").is_file()
            || bounded_output("/usr/local/bin/stealth", &["version"])
                .is_some_and(|version| version.trim_ascii_end() == b"stealth-transparent-v1"));
    if active {
        println!(
            "{}\nStealth mode active · Tor routing",
            runtime::home()?
                .join(".config/waybar/icons/stealth.svg")
                .display()
        );
    } else {
        print!("\n\n");
    }
    Ok(())
}

fn tray_item_path(item: &str) -> Option<(&str, &str)> {
    let (name, path) = match item.find('/') {
        Some(index) => item.split_at(index),
        None => (item, "/StatusNotifierItem"),
    };
    (gio::dbus_is_name(name) && glib::Variant::is_object_path(path)).then_some((name, path))
}

fn visible_warp_item(properties: &HashMap<String, glib::Variant>) -> bool {
    let field = |key| properties.get(key).and_then(glib::Variant::str);
    // The vendor generates the item ID. Its public tray title is stable.
    field("Title") == Some("warp-taskbar")
        && field("Id").is_some_and(|id| !id.is_empty())
        && field("Category").is_some_and(|category| !category.is_empty())
        && field("Status").is_some_and(|status| {
            status.eq_ignore_ascii_case("Active") || status.eq_ignore_ascii_case("NeedsAttention")
        })
}

async fn warp_tray_registered() -> Result<bool> {
    let connection = bus::session().await?;
    let deadline = Instant::now() + TRAY_WAIT;
    let (watcher,) = bus::call(
        &connection,
        TRAY_WATCHER,
        "/StatusNotifierWatcher",
        "org.freedesktop.DBus.Properties",
        "GetAll",
        Some(&(TRAY_WATCHER,).to_variant()),
        TRAY_WAIT.as_millis() as i32,
    )
    .await?
    .get::<(HashMap<String, glib::Variant>,)>()
    .ok_or_else(|| err("Invalid status notifier registration"))?;
    if watcher
        .get("IsStatusNotifierHostRegistered")
        .and_then(glib::Variant::get::<bool>)
        != Some(true)
    {
        return Ok(false);
    }
    let items = watcher
        .get("RegisteredStatusNotifierItems")
        .and_then(glib::Variant::get::<Vec<String>>)
        .ok_or_else(|| err("Invalid status notifier item list"))?;
    for (service, path) in items.iter().filter_map(|item| tray_item_path(item)) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let properties = bus::call(
            &connection,
            service,
            path,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            Some(&(TRAY_ITEM,).to_variant()),
            remaining.as_millis().clamp(1, 250) as i32,
        )
        .await;
        if let Ok(properties) = properties
            && let Some((properties,)) = properties.get::<(HashMap<String, glib::Variant>,)>()
            && visible_warp_item(&properties)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn warp_indicator() {
    if !active_unit("warp-svc.service").await.unwrap_or(false)
        || warp_tray_registered().await.unwrap_or(false)
    {
        println!("{}", json!({"text": ""}));
        return;
    }
    let status = bounded_output("warp-cli", &["--accept-tos", "--json", "status"])
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let state = status
        .as_ref()
        .and_then(|status| status.get("status")?.as_str())
        .filter(|state| !state.is_empty())
        .unwrap_or("Unknown");
    let class = if state == "Connected" {
        "connected"
    } else {
        "running"
    };
    println!(
        "{}",
        json!({"text": "󰖟", "tooltip": format!("Cloudflare WARP · {state}"), "class": class})
    );
}

pub async fn run(command: &str, args: &[OsString]) -> Result<i32> {
    match command {
        "fullscreen-toggle" if args.is_empty() => {
            ipc::action(json!({"FullscreenWindow": {"id": null}}))?;
        }
        "app-launcher" if args.is_empty() => launcher::run().await?,
        "blueman-manager" => blueman_manager(args)?,
        "launch-default" if args.len() == 1 => {
            launcher::launch_default(&args[0].to_string_lossy()).await?;
        }
        "menu-toggle" => return menu_toggle(args),
        "password-prompt" => return password_prompt(args).await,
        "power-menu" if args.is_empty() => power_menu().await?,
        "power-menu-action" => {
            let action = match args {
                [action] => action.to_str(),
                _ => None,
            }
            .filter(|action| {
                matches!(
                    *action,
                    "lock" | "suspend" | "logout" | "shutdown" | "reboot" | "restart"
                )
            })
            .ok_or_else(|| err("Invalid power action"))?;
            terminate_named("wlogout");
            power_action(action).await?;
        }
        "start-idle" if args.is_empty() => {
            // Keep the restart synchronous so fallback only runs when the service fails.
            let _ = Command::new("dbus-update-activation-environment")
                .args([
                    "--systemd",
                    "WAYLAND_DISPLAY",
                    "XDG_CURRENT_DESKTOP",
                    "XDG_SESSION_TYPE",
                    "XDG_SESSION_ID",
                    "NIRI_SOCKET",
                ])
                .status();
            if !Command::new("systemctl")
                .args(["--user", "restart", "hypridle.service"])
                .status()
                .is_ok_and(|status| status.success())
            {
                return Err(Command::new("hypridle").exec().into());
            }
        }
        "start-tray-applets" if args.is_empty() => start_tray_applets()?,
        "stop-waybar" if args.is_empty() => {
            terminate_named("waybar");
        }
        "start-waybar" if args.is_empty() => start_waybar(false)?,
        "start-waybar" if args == ["--restart"] => start_waybar(true)?,
        "stealth-indicator" if args.is_empty() => stealth_indicator().await?,
        "warp-indicator" if args.is_empty() => warp_indicator().await,
        _ => return Err(err(format!("Invalid arguments for {command}"))),
    }
    Ok(0)
}
