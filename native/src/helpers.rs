use crate::{Result, bus, caffeine, err, ipc, runtime, session};
use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read, Seek, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::{CommandExt, ExitStatusExt},
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

fn menu_toggle(args: &[OsString]) -> Result<i32> {
    let Some((program, arguments)) = args.split_first() else {
        return Err(err("Usage: menu-toggle PROGRAM [arguments]"));
    };
    let lock = runtime::lock_blocking("hype-menu-toggle.lock")?;
    if terminate_named("wofi") {
        return Ok(0);
    }
    let mut child = Command::new(program).args(arguments).spawn()?;
    // Keep a second press queued until Wofi starts or the command exits.
    while named_processes("wofi").next().is_none() {
        if let Some(status) = child.try_wait()? {
            return Ok(exit_code(status));
        }
        thread::sleep(Duration::from_millis(20));
    }
    drop(lock);
    Ok(exit_code(child.wait()?))
}

fn password_prompt(args: &[OsString]) -> Result<i32> {
    let config = match env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        Some(path) => path.into(),
        None => runtime::home()?.join(".config"),
    };
    let style = config.join("wofi/style.css");
    let mut prompt_style = tempfile::Builder::new()
        .prefix("wofi-password.")
        .suffix(".css")
        .tempfile_in(runtime::dir()?)?;
    io::copy(&mut fs::File::open(style)?, &mut prompt_style)?;
    prompt_style.write_all(
        br#"#input { margin: 2px 20px; min-height: 60px; font-feature-settings: "liga=0,calt=0"; }
#scroll, #inner-box { margin: 0; padding: 0; min-height: 0; }
#entry { margin: 0; padding: 0; min-height: 0; font-size: 0; }
"#,
    )?;
    let status = Command::new("wofi")
        .args([
            "--dmenu",
            "--password",
            "--height",
            "64",
            "--cache-file",
            "/dev/null",
            "--style",
        ])
        .arg(prompt_style.path())
        .args(args)
        .status()?;
    Ok(exit_code(status))
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
    let mut picker = Command::new("wofi")
        .args([
            "--dmenu",
            "--no-custom-entry",
            "--cache-file",
            "/dev/null",
            "--allow-markup",
            "--define",
            "dmenu-print_line_num=true",
            "-p",
            "Power",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let options = [
        ("\u{f033e}", "Lock"),
        ("\u{f04b2}", "Sleep"),
        ("\u{f0343}", "Logout"),
        ("\u{f0425}", "Shutdown"),
        ("\u{f0709}", "Reboot"),
    ];
    let mut input = picker.stdin.take().expect("Wofi stdin is piped");
    let feed = options.into_iter().try_for_each(|(icon, label)| {
        writeln!(
            input,
            "<span font_family='JetBrainsMono Nerd Font'>{icon}</span>  {label}"
        )
    });
    drop(input);
    let picked = picker.wait_with_output()?;
    if feed.is_err() || !picked.status.success() {
        return Ok(());
    }
    let choice = picked.stdout.trim_ascii_end();
    match choice {
        b"0" => return session::run("lock-screen", &[]).await,
        b"1" => return session::run("suspend-now", &[]).await,
        b"2" | b"3" | b"4" => {
            let _ = caffeine::run("caffeine", &["stop".into()]).await;
        }
        _ => return Ok(()),
    }
    match choice {
        b"2" => ipc::action(json!({"Quit": {"skip_confirmation": true}})),
        b"3" => logind_power("PowerOff").await,
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

fn start_tray_applets() -> Result<()> {
    let Some(_launching) = runtime::lock("hype-tray-applets.lock", Duration::ZERO)? else {
        return Ok(());
    };
    for (name, args) in [
        ("nm-applet", &["--indicator"][..]),
        ("blueman-applet", &[][..]),
        ("warp-taskbar", &[][..]),
    ] {
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

async fn warp_indicator() {
    if named_processes("warp-taskbar").next().is_some()
        || !active_unit("warp-svc.service").await.unwrap_or(false)
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
        "menu-toggle" => return menu_toggle(args),
        "password-prompt" => return password_prompt(args),
        "power-menu" if args.is_empty() => power_menu().await?,
        "start-tray-applets" if args.is_empty() => start_tray_applets()?,
        "start-waybar" if args.is_empty() => start_waybar(false)?,
        "start-waybar" if args == ["--restart"] => start_waybar(true)?,
        "stealth-indicator" if args.is_empty() => stealth_indicator().await?,
        "warp-indicator" if args.is_empty() => warp_indicator().await,
        _ => return Err(err(format!("Invalid arguments for {command}"))),
    }
    Ok(0)
}
