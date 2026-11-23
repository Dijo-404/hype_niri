use hype_install::{
    Installer, ProcessRunner, Result,
    core::{InstallLock, runtime_directory},
    error,
};
use std::{os::fd::AsRawFd, path::PathBuf};

#[derive(Debug, PartialEq)]
enum Mode {
    Install,
    Oomd,
    Stealth(PathBuf),
    Phase(String),
    Help,
}
#[derive(Debug, PartialEq)]
struct Options {
    source: PathBuf,
    mode: Mode,
}
fn parse(args: &[String]) -> Result<Options> {
    let mut source = std::env::current_dir()?;
    let mut mode = Mode::Install;
    let mut source_seen = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                if source_seen {
                    return Err(error("Duplicate --source argument"));
                }
                source_seen = true;
                index += 1;
                source = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| error("Missing --source path"))?,
                );
            }
            "--setup-oomd" => {
                if mode != Mode::Install {
                    return Err(error("Choose one installer operation"));
                }
                mode = Mode::Oomd;
                index += 1;
                source = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| error("Missing OOM source path"))?,
                );
            }
            "--install-stealth" => {
                if mode != Mode::Install {
                    return Err(error("Choose one installer operation"));
                }
                index += 1;
                let payload = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| error("Missing Stealth payload path"))?,
                );
                mode = Mode::Stealth(payload);
            }
            "--phase" => {
                if mode != Mode::Install {
                    return Err(error("Choose one installer operation"));
                }
                index += 1;
                let name = args.get(index).ok_or_else(|| error("Missing phase name"))?;
                if !matches!(
                    name.as_str(),
                    "packages"
                        | "user-dirs"
                        | "desktop-integrations"
                        | "lazyvim"
                        | "tray-icons"
                        | "docker-socket"
                        | "backup"
                        | "configs"
                        | "gtk"
                        | "shell"
                        | "system"
                        | "logind"
                        | "firewall"
                        | "warp"
                        | "stealth"
                        | "validate"
                ) {
                    return Err(error(format!("Unknown phase: {name}")));
                }
                mode = Mode::Phase(name.clone());
            }
            "--help" | "-h" => {
                mode = Mode::Help;
            }
            other => return Err(error(format!("Unknown argument: {other}. Use --help."))),
        }
        index += 1;
    }
    Ok(Options { source, mode })
}
fn entry() -> Result<()> {
    let options = parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if options.mode == Mode::Help {
        println!(
            "Usage: hype-install [--source REPOSITORY] [--phase NAME]\n       hype-install --setup-oomd REPOSITORY\n       sudo hype-install --install-stealth PAYLOAD\n\nPhases: packages user-dirs desktop-integrations lazyvim tray-icons docker-socket backup configs gtk shell system logind firewall warp stealth validate\nRun interactively as a regular Arch Linux user with sudo and yay installed."
        );
        return Ok(());
    }
    let uid = unsafe { libc::geteuid() };
    if matches!(options.mode, Mode::Install | Mode::Phase(_)) && uid == 0 {
        return Err(error(
            "Run as your regular user; this installer uses sudo when required",
        ));
    }
    if matches!(options.mode, Mode::Stealth(_)) && uid != 0 {
        return Err(error("Stealth installation requires sudo"));
    }
    unsafe {
        libc::umask(0o077);
    }
    if !matches!(options.mode, Mode::Stealth(_)) && unsafe { libc::isatty(libc::STDIN_FILENO) } != 1
    {
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| error("An interactive terminal is required"))?;
        if unsafe { libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    let source = options.source.canonicalize()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| error("HOME is unset"))?;
    let mut installer = Installer {
        source,
        home,
        root: PathBuf::from("/"),
        backup: None,
        build_dir: None,
        runner: Box::new(ProcessRunner),
    };
    match options.mode {
        Mode::Install => {
            let _lock = InstallLock::acquire(&runtime_directory()?)?;
            installer.run_all()
        }
        Mode::Oomd => installer.setup_oomd(),
        Mode::Stealth(payload) => installer.install_stealth(&payload),
        Mode::Phase(name) => {
            let _lock = InstallLock::acquire(&runtime_directory()?)?;
            match name.as_str() {
                "packages" => installer.install_packages(),
                "user-dirs" => installer.setup_user_dirs(),
                "desktop-integrations" => installer.setup_desktop_integrations(),
                "lazyvim" => installer.setup_lazyvim(),
                "tray-icons" => installer.setup_tray_icons(),
                "docker-socket" => installer.configure_docker_socket(),
                "backup" => installer.backup_configs(),
                "configs" => installer.copy_configs(),
                "gtk" => installer.setup_gtk(),
                "shell" => installer.setup_shell(),
                "system" => installer.setup_system(),
                "logind" => installer.setup_logind(),
                "firewall" => installer.setup_firewall(),
                "warp" => installer.setup_cloudflare(),
                "stealth" => installer.setup_stealth(),
                "validate" => installer.validate(),
                _ => unreachable!(),
            }
        }
        Mode::Help => unreachable!(),
    }
}
fn main() {
    if let Err(cause) = entry() {
        eprintln!("hype-install: {cause}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).into()).collect()
    }
    #[test]
    fn local_checkout_source_remains_one_argument() {
        let parsed = parse(&arguments(&["--source", "/tmp/two words"])).unwrap();
        assert_eq!(parsed.source, PathBuf::from("/tmp/two words"));
        assert_eq!(parsed.mode, Mode::Install);
    }
    #[test]
    fn unknown_or_incomplete_arguments_fail_before_install() {
        for args in [
            vec!["--source"],
            vec!["--unknown"],
            vec!["--install-stealth"],
            vec!["--source", "a", "--source", "b"],
        ] {
            assert!(parse(&arguments(&args)).is_err());
        }
    }
    #[test]
    fn focused_root_mode_preserves_exact_payload_path() {
        let options = parse(&arguments(&[
            "--install-stealth",
            "/tmp/payload with spaces",
        ]))
        .unwrap();
        assert_eq!(
            options.mode,
            Mode::Stealth(PathBuf::from("/tmp/payload with spaces"))
        );
    }
    #[test]
    fn manual_phase_interface_accepts_only_named_operations() {
        assert!(parse(&arguments(&["--phase", "user-dirs"])).is_ok());
        assert!(parse(&arguments(&["--phase", "arbitrary-command"])).is_err());
    }
}
