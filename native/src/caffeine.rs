use crate::{Result, bus, err, notify, runtime};
use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    fs,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Holder {
    pid: u32,
    start: u64,
    token: String,
}

fn parse_starttime(stat: &str) -> Option<u64> {
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

const LOGIN: &str = "org.freedesktop.login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIN_PATH: &str = "/org/freedesktop/login1";

/// logind keeps the inhibitor only while this received descriptor remains open.
pub(crate) async fn inhibit(
    conn: &gio::DBusConnection,
    what: &str,
    who: &str,
    why: &str,
    mode: &str,
) -> Result<OwnedFd> {
    let (reply, fds) = conn
        .call_with_unix_fd_list_future(
            Some(LOGIN),
            LOGIN_PATH,
            MANAGER,
            "Inhibit",
            Some(&(what, who, why, mode).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5_000,
            None::<&gio::UnixFDList>,
        )
        .await?;
    let handle = reply
        .try_child_value(0)
        .and_then(|v| v.get::<glib::variant::Handle>())
        .ok_or_else(|| err("logind returned an invalid inhibitor handle"))?;
    let fd = fds
        .ok_or_else(|| err("logind returned no inhibitor descriptor"))?
        .get(handle.0)?;
    // Do not leak inhibitors into launched applications.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(fd)
}

fn read_holder(dir: &Path) -> Option<Holder> {
    serde_json::from_slice(&fs::read(dir.join("caffeine_pid")).ok()?).ok()
}
fn legacy_holder(pid: u32, dir: &Path) -> Option<std::path::PathBuf> {
    let proc = Path::new("/proc").join(pid.to_string());
    if fs::metadata(&proc).ok()?.uid() != runtime::uid()
        || !executable_matches(
            &fs::read_link(proc.join("exe")).ok()?,
            &fs::canonicalize("/usr/bin/systemd-inhibit").ok()?,
        )
    {
        return None;
    }
    let command = fs::read(proc.join("cmdline")).ok()?;
    let args: Vec<_> = command
        .strip_suffix(&[0])?
        .split(|&byte| byte == 0)
        .collect();
    let script = runtime::home()
        .ok()?
        .join(".config/scripts/caffeine-control.sh");
    let [
        _,
        b"--what=idle",
        b"--who=Caffeine Mode",
        b"--why=User requested stay awake",
        b"--mode=block",
        b"--",
        b"bash",
        path,
        b"hold",
        readiness,
    ] = args.as_slice()
    else {
        return None;
    };
    if *path != script.as_os_str().as_encoded_bytes() {
        return None;
    }
    let readiness = Path::new(std::ffi::OsStr::from_bytes(readiness));
    (readiness.parent() == Some(dir)
        && readiness
            .file_name()?
            .as_encoded_bytes()
            .starts_with(b"caffeine-ready."))
    .then(|| readiness.to_owned())
}
fn ready_path(dir: &Path, token: &str) -> PathBuf {
    dir.join(format!("caffeine-ready.{token}"))
}

fn legacy_holders(dir: &Path) -> Result<Vec<(u32, PathBuf)>> {
    let mut holders = Vec::new();
    // An earlier build could erase the PID file while the legacy inhibitor remained open.
    for entry in fs::read_dir("/proc")?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if fs::read_to_string(entry.path().join("comm"))
            .is_ok_and(|name| name.trim_end() == "systemd-inhibit")
            && let Some(readiness) = legacy_holder(pid, dir)
        {
            holders.push((pid, readiness));
        }
    }
    Ok(holders)
}
fn process_start(pid: u32) -> Option<u64> {
    parse_starttime(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}
fn executable_matches(running: &Path, expected: &Path) -> bool {
    running == expected
        || running
            .as_os_str()
            .as_encoded_bytes()
            .strip_suffix(b" (deleted)")
            == Some(expected.as_os_str().as_encoded_bytes())
}
fn holder_matches(holder: &Holder, exe: &Path) -> bool {
    if !valid_token(&holder.token) || process_start(holder.pid) != Some(holder.start) {
        return false;
    }
    let proc = PathBuf::from(format!("/proc/{}", holder.pid));
    if !fs::metadata(&proc).is_ok_and(|m| m.uid() == unsafe { libc::geteuid() }) {
        return false;
    }
    if !fs::read_link(proc.join("exe")).is_ok_and(|p| executable_matches(&p, exe)) {
        return false;
    }
    let Ok(cmd) = fs::read(proc.join("cmdline")) else {
        return false;
    };
    let args: Vec<&[u8]> = cmd.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
    args.len() == 4
        && args[1] == b"caffeine"
        && args[2] == b"hold"
        && args[3] == holder.token.as_bytes()
}
fn acquired(dir: &Path, holder: &Holder, exe: &Path) -> bool {
    holder_matches(holder, exe)
        && fs::read(ready_path(dir, &holder.token))
            .ok()
            .and_then(|b| serde_json::from_slice::<Holder>(&b).ok())
            .as_ref()
            == Some(holder)
}

// Open the process handle before checking identity, then signal that exact process.
fn signal_holder(holder: &Holder, exe: &Path) -> Result<bool> {
    let Some(process) = runtime::Process::open(holder.pid)? else {
        return Ok(false);
    };
    if !holder_matches(holder, exe) {
        return Ok(false);
    }
    process.terminate()?;
    Ok(true)
}
fn remove(path: impl AsRef<Path>) {
    let _ = fs::remove_file(path);
}

async fn stop(dir: &Path) -> Result<()> {
    if let Some(holder) = read_holder(dir) {
        let exe = std::env::current_exe()?;
        if signal_holder(&holder, &exe)? {
            // The holder deletes readiness only after closing its inhibitor FD.
            let deadline = Instant::now() + Duration::from_secs(2);
            while holder_matches(&holder, &exe) && ready_path(dir, &holder.token).exists() {
                if Instant::now() >= deadline {
                    return Err(err("Caffeine holder did not release its inhibitor"));
                }
                glib::timeout_future(Duration::from_millis(20)).await;
            }
        }
        if valid_token(&holder.token) {
            remove(ready_path(dir, &holder.token));
        }
    }
    for (pid, readiness) in legacy_holders(dir)? {
        if let Some(process) = runtime::Process::open(pid)?
            && legacy_holder(pid, dir).as_ref() == Some(&readiness)
        {
            process.terminate()?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while !process.exited()? {
                if Instant::now() >= deadline {
                    return Err(err("Legacy Caffeine holder did not release its inhibitor"));
                }
                glib::timeout_future(Duration::from_millis(20)).await;
            }
        }
    }
    remove(dir.join("caffeine_pid"));
    remove(dir.join("caffeine_state"));
    Ok(())
}

async fn start(dir: &Path) -> Result<()> {
    stop(dir).await?;
    let token = format!("{}-{:016x}", std::process::id(), rand::random::<u64>());
    let ready = ready_path(dir, &token);
    let mut child = Command::new(std::env::current_exe()?)
        .args(["caffeine", "hold", &token])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if let Some(holder) = read_holder(dir)
            && holder.pid == child.id()
            && holder.token == token
            && acquired(dir, &holder, &std::env::current_exe()?)
        {
            if let Err(error) = runtime::atomic_write(&dir.join("caffeine_state"), token.as_bytes())
            {
                let _ = child.kill();
                let _ = child.wait();
                remove(&ready);
                remove(dir.join("caffeine_pid"));
                return Err(error);
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        glib::timeout_future(Duration::from_millis(20)).await;
    }
    // An unreaped child cannot have its PID reused.
    let _ = child.kill();
    let _ = child.wait();
    remove(ready);
    remove(dir.join("caffeine_pid"));
    remove(dir.join("caffeine_state"));
    Err(err("Could not acquire the idle inhibitor"))
}

async fn hold(dir: &Path, token: &str) -> Result<()> {
    if !valid_token(token) {
        return Err(err("Invalid caffeine holder token"));
    }
    let Some(_single) = runtime::lock("caffeine-holder.lock", Duration::ZERO)? else {
        return Err(err("Caffeine holder already running"));
    };
    let alive = Rc::new(Cell::new(true));
    let mut signals = Vec::new();
    for signum in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let alive = alive.clone();
        signals.push(glib_unix::unix_signal_add_local(signum, move || {
            alive.set(false);
            glib::ControlFlow::Continue
        }));
    }
    let conn = bus::system().await?;
    let owner_alive = alive.clone();
    let _owner = conn.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(LOGIN),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if signal
                .parameters
                .get::<(String, String, String)>()
                .is_some_and(|(_, old, _)| !old.is_empty())
            {
                owner_alive.set(false);
            }
        },
    );
    let fd = inhibit(
        &conn,
        "idle",
        "Caffeine Mode",
        "User requested stay awake",
        "block",
    )
    .await?;
    let holder = Holder {
        pid: std::process::id(),
        start: process_start(std::process::id())
            .ok_or_else(|| err("Could not identify caffeine holder"))?,
        token: token.into(),
    };
    let bytes = serde_json::to_vec(&holder)?;
    runtime::atomic_write(&dir.join("caffeine_pid"), &bytes)?;
    runtime::atomic_write(&ready_path(dir, token), &bytes)?;
    while alive.get() && !conn.is_closed() {
        glib::timeout_future(Duration::from_millis(20)).await;
    }
    drop(fd);
    remove(ready_path(dir, token));
    for signal in signals {
        signal.remove();
    }
    // Startup/stop serialize with cleanup; close the inhibitor before waiting.
    if let Some(_lock) = runtime::lock("caffeine.lock", Duration::from_secs(2))?
        && read_holder(dir).as_ref() == Some(&holder)
    {
        remove(dir.join("caffeine_pid"));
        if fs::read_to_string(dir.join("caffeine_state"))
            .ok()
            .as_deref()
            == Some(token)
        {
            remove(dir.join("caffeine_state"));
        }
    }
    let _ = runtime::signal_waybar(15);
    Ok(())
}

fn print_status(active: bool) {
    println!(
        "{}",
        if active {
            serde_json::json!({"text":"󰅶", "tooltip":"Caffeine: On", "class":"activated"})
        } else {
            serde_json::json!({"text":"󰾪", "tooltip":"Caffeine: Off", "class":"deactivated"})
        }
    );
}

pub async fn run(_command: &str, args: &[String]) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("status");
    let dir = runtime::dir()?;
    if action == "hold" {
        if args.len() != 2 {
            return Err(err("Usage: caffeine hold TOKEN"));
        }
        return hold(&dir, &args[1]).await;
    }
    if args.len() > 1 || !["status", "stop", "toggle"].contains(&action) {
        return Err(err("Usage: caffeine [status|toggle|stop]"));
    }
    let Some(_lock) = runtime::lock("caffeine.lock", Duration::from_secs(2))? else {
        return Err(err("Caffeine is busy"));
    };
    if action == "stop" {
        stop(&dir).await?;
        let _ = runtime::signal_waybar(15);
        return Ok(());
    }
    let current = read_holder(&dir);
    let active = current.as_ref().is_some_and(|holder| {
        fs::read_to_string(dir.join("caffeine_state"))
            .ok()
            .as_deref()
            == Some(holder.token.as_str())
            && std::env::current_exe().is_ok_and(|exe| acquired(&dir, holder, &exe))
    }) || legacy_holders(&dir)?
        .iter()
        .any(|(_, ready)| fs::metadata(ready).is_ok_and(|m| m.is_file() && m.len() > 0));
    let mut active = active;
    if action == "toggle" {
        if active {
            stop(&dir).await?;
            active = false;
            let _ = notify::send(
                2002,
                "󰾪  Caffeine Mode Deactivated",
                "Idle lock and display sleep restored",
                None,
                "device",
            )
            .await;
        } else {
            active = start(&dir).await.is_ok();
            let (title, body) = if active {
                (
                    "󰅶  Caffeine Mode Active",
                    "Idle lock and display sleep paused",
                )
            } else {
                (
                    "Caffeine unavailable",
                    "Could not acquire the idle inhibitor",
                )
            };
            let _ = notify::send(2002, title, body, None, "device").await;
        }
        let _ = runtime::signal_waybar(15);
    } else if !active {
        stop(&dir).await?;
    }
    print_status(active);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proc_starttime_handles_parentheses_and_spaces_in_process_name() {
        let stat = "123 (a tricky ) process) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 991 0";
        assert_eq!(parse_starttime(stat), Some(991));
        assert_eq!(parse_starttime("123 bad"), None);
        assert_eq!(parse_starttime("123 (bad) S"), None);
    }
    #[test]
    fn holder_tokens_cannot_escape_runtime_directory() {
        assert!(valid_token("1234-456-abc"));
        for token in ["", "../state", "abc/def", "hello\nworld", &"a".repeat(129)] {
            assert!(!valid_token(token), "{token}");
        }
    }
    #[test]
    fn unrelated_recorded_process_and_reused_starttime_are_rejected() {
        let current = Holder {
            pid: std::process::id(),
            start: process_start(std::process::id()).unwrap(),
            token: "unit-test".into(),
        };
        assert!(!holder_matches(&current, &std::env::current_exe().unwrap()));
        let stale = Holder {
            start: current.start.saturating_add(1),
            ..current
        };
        assert!(!holder_matches(&stale, &std::env::current_exe().unwrap()));
    }

    fn descriptor_fixture(missing_fd: bool) {
        use std::{cell::RefCell, io::Read, os::unix::net::UnixStream};
        let context = glib::MainContext::new();
        context.with_thread_default(|| context.block_on(async {
            let server = gio::DBusServer::new_sync("unix:tmpdir=/tmp", gio::DBusServerFlags::AUTHENTICATION_ALLOW_ANONYMOUS, &gio::dbus_generate_guid(), None, None::<&gio::Cancellable>).unwrap();
            let retained = Rc::new(RefCell::new(Vec::new()));
            let connections = retained.clone();
            let readers = Rc::new(RefCell::new(Vec::<UnixStream>::new()));
            let peers = readers.clone();
            server.connect_new_connection(move |_, connection| {
                connection.set_exit_on_close(false);
                let info = gio::DBusNodeInfo::for_xml("<node><interface name='org.freedesktop.login1.Manager'><method name='Inhibit'><arg type='s' direction='in'/><arg type='s' direction='in'/><arg type='s' direction='in'/><arg type='s' direction='in'/><arg type='h' direction='out'/></method></interface></node>").unwrap();
                let peers = peers.clone();
                connection.register_object(LOGIN_PATH, &info.interfaces()[0]).method_call(move |_, _, _, _, method, args, invocation| {
                    if method != "Inhibit" || args.get::<(String, String, String, String)>() != Some(("idle".into(), "Caffeine Mode".into(), "User requested stay awake".into(), "block".into())) {
                        invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Expected idle-only block inhibitor");
                        return;
                    }
                    if missing_fd {
                        invocation.return_value(Some(&(glib::variant::Handle(0),).to_variant()));
                    } else {
                        let (read, write) = UnixStream::pair().unwrap();
                        read.set_nonblocking(true).unwrap();
                        peers.borrow_mut().push(read);
                        let fds = gio::UnixFDList::new();
                        let handle = fds.append(&write).unwrap();
                        invocation.return_value_with_unix_fd_list(Some(&(glib::variant::Handle(handle),).to_variant()), Some(&fds));
                    }
                }).build().unwrap();
                connections.borrow_mut().push(connection.clone());
                true
            });
            server.start();
            let connection = gio::DBusConnection::for_address_future(&server.client_address(), gio::DBusConnectionFlags::AUTHENTICATION_CLIENT, None).await.unwrap();
            connection.set_exit_on_close(false);
            let result = inhibit(&connection, "idle", "Caffeine Mode", "User requested stay awake", "block").await;
            if missing_fd {
                assert!(result.is_err(), "Missing inhibitor descriptor was reported acquired");
            } else {
                let fd = result.unwrap();
                assert_ne!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
                let mut byte = [0];
                assert_eq!(readers.borrow_mut()[0].read(&mut byte).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
                drop(fd);
                assert_eq!(readers.borrow_mut()[0].read(&mut byte).unwrap(), 0, "Inhibitor descriptor leaked after drop");
            }
            connection.close_future().await.unwrap();
            server.stop();
        })).unwrap();
    }
    #[test]
    fn native_idle_inhibitor_keeps_descriptor_until_explicit_drop() {
        descriptor_fixture(false);
    }
    #[test]
    fn native_inhibitor_rejects_success_reply_without_descriptor() {
        descriptor_fixture(true);
    }
    #[test]
    fn holder_remains_identifiable_after_atomic_binary_replacement() {
        assert!(executable_matches(
            Path::new("/opt/hype-control (deleted)"),
            Path::new("/opt/hype-control")
        ));
        assert!(!executable_matches(
            Path::new("/other/hype-control (deleted)"),
            Path::new("/opt/hype-control")
        ));
    }
}
