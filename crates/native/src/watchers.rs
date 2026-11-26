use crate::{Result, err, runtime};
use std::{
    ffi::OsStr,
    fs,
    os::linux::net::SocketAddrExt,
    os::unix::{
        fs::MetadataExt,
        net::{SocketAddr, UnixDatagram},
    },
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const USER_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_ID",
    "NIRI_SOCKET",
];

pub(crate) fn ready(kind: Watcher) -> Result<()> {
    let Some(address) = std::env::var_os("NOTIFY_SOCKET") else {
        return Ok(());
    };
    let address = if let Some(name) = address.as_encoded_bytes().strip_prefix(b"@") {
        SocketAddr::from_abstract_name(name)?
    } else {
        SocketAddr::from_pathname(address)?
    };
    UnixDatagram::unbound()?.send_to_addr(b"READY=1", &address)?;
    // The service can reach readiness on a later systemd restart as well as the initial start.
    std::thread::spawn(move || {
        if let Err(error) = finish_handoff(kind.unit()) {
            eprintln!(
                "Could not retire the previous {} watcher: {error}",
                kind.unit()
            );
        }
    });
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum Watcher {
    Sleep,
    Clipboard,
}

impl Watcher {
    fn unit(self) -> &'static str {
        match self {
            Self::Sleep => SLEEP,
            Self::Clipboard => CLIPBOARD,
        }
    }
}

const SLEEP: &str = "hype-sleep-watch.service";
const CLIPBOARD: &str = "hype-clipboard-watch.service";
const SWAYIDLE_HOOK: &[u8] = br#""$HOME/.config/scripts/prepare-sleep.sh""#;

fn systemctl(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("systemctl").arg("--user").args(args).output()
}

fn start_unit(unit: &str) -> Result<()> {
    let output = systemctl(&["start", unit])?;
    if !output.status.success() {
        return Err(err(format!(
            "Could not start {unit}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn old_command(unit: &str, command: &[u8], exe: &Path, scripts: &Path) -> bool {
    let Some((program, args)) = command.strip_suffix(&[0]).and_then(|cmd| {
        let mut parts = cmd.split(|&byte| byte == 0);
        Some((parts.next()?, parts.collect::<Vec<_>>()))
    }) else {
        return false;
    };
    if unit == SLEEP
        && (exe.file_name() == Some(OsStr::new("swayidle"))
            || exe.file_name() == Some(OsStr::new("swayidle (deleted)")))
        && args
            == [
                b"-w".as_slice(),
                b"-C",
                b"/dev/null",
                b"before-sleep",
                SWAYIDLE_HOOK,
            ]
    {
        return true;
    }
    if unit == CLIPBOARD
        && exe
            .file_name()
            .is_some_and(|name| name == "wl-paste" || name == "wl-paste (deleted)")
        && matches!(
            args.as_slice(),
            [
                b"--type",
                b"text" | b"image",
                b"--watch",
                b"cliphist",
                b"store"
            ]
        )
    {
        return true;
    }
    let binary = scripts.join("hype-control");
    let running = exe.as_os_str().as_encoded_bytes();
    let expected = binary.as_os_str().as_encoded_bytes();
    program == expected
        && (running == expected || running.strip_suffix(b" (deleted)") == Some(expected))
        && match unit {
            SLEEP => args == [b"prepare-sleep".as_slice(), b"--watch"],
            CLIPBOARD => args == [b"clipboard".as_slice(), b"watch"],
            _ => false,
        }
}

fn retire_previous(unit: &str, current_pid: u32, scripts: &Path) -> Result<()> {
    let mut stopped = Vec::new();
    for entry in fs::read_dir("/proc")?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == current_pid || !entry.metadata().is_ok_and(|m| m.uid() == runtime::uid()) {
            continue;
        }
        let path = entry.path();
        let (Ok(command), Ok(exe)) = (
            fs::read(path.join("cmdline")),
            fs::read_link(path.join("exe")),
        ) else {
            continue;
        };
        if !old_command(unit, &command, &exe, scripts) {
            continue;
        }
        if let Some(process) = runtime::Process::open(pid)?
            && fs::read(path.join("cmdline")).is_ok_and(|value| value == command)
            && fs::read_link(path.join("exe")).is_ok_and(|value| value == exe)
        {
            process.terminate()?;
            stopped.push(process);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    for process in stopped {
        while !process.exited()? {
            if Instant::now() >= deadline {
                return Err(err(format!("The previous {unit} watcher did not exit")));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}

fn finish_handoff(unit: &str) -> Result<()> {
    let scripts = runtime::home()?.join(".config/scripts");
    retire_previous(unit, std::process::id(), &scripts)?;
    let old_hook = scripts.join("prepare-sleep.sh");
    if unit == SLEEP
        && fs::read_link(&old_hook).is_ok_and(|target| target == Path::new("hype-control"))
        && let Err(error) = fs::remove_file(old_hook)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.into());
    }
    Ok(())
}

pub fn start() -> Result<()> {
    let mut import = vec!["import-environment"];
    import.extend(
        USER_ENV
            .iter()
            .copied()
            .filter(|key| std::env::var_os(key).is_some()),
    );
    if import.len() == 1 || !systemctl(&import)?.status.success() {
        return Err(err(
            "Could not import the desktop environment for Hype Niri watchers",
        ));
    }
    let sleep = start_unit(SLEEP);
    let clipboard = start_unit(CLIPBOARD);
    sleep.and(clipboard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_matches_only_hype_watchers() {
        let scripts = Path::new("/home/dj/.config/scripts");
        let swayidle = Path::new("/usr/bin/swayidle");
        let command = [
            b"swayidle".as_slice(),
            b"-w",
            b"-C",
            b"/dev/null",
            b"before-sleep",
            SWAYIDLE_HOOK,
            b"",
        ]
        .join(&0);
        assert!(old_command(SLEEP, &command, swayidle, scripts));
        assert!(!old_command(CLIPBOARD, &command, swayidle, scripts));
        assert!(!old_command(
            SLEEP,
            &command,
            Path::new("/usr/bin/sh"),
            scripts
        ));
        for (unit, command) in [
            (
                SLEEP,
                b"/home/dj/.config/scripts/hype-control\0prepare-sleep\0--watch\0".as_slice(),
            ),
            (
                CLIPBOARD,
                b"/home/dj/.config/scripts/hype-control\0clipboard\0watch\0",
            ),
        ] {
            assert!(old_command(
                unit,
                command,
                &scripts.join("hype-control"),
                scripts
            ));
            assert!(!old_command(
                unit,
                command,
                &scripts.join("hype-control"),
                Path::new("/other")
            ));
        }
    }
}
