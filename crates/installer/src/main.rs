use hype_install::{
    Installer, ProcessRunner, Result,
    core::{InstallLock, runtime_directory},
    error,
};
use std::{io::Write, os::fd::AsRawFd, path::PathBuf};
mod tui;

#[derive(Debug, PartialEq)]
enum Mode {
    Menu,
    Install,
    Update,
    SetupCli,
    Oomd,
    Stealth(PathBuf),
    Phase(String),
    Help,
}
#[derive(Debug, PartialEq)]
struct Options {
    source: PathBuf,
    source_seen: bool,
    mode: Mode,
    plain: bool,
}
fn parse(args: &[String]) -> Result<Options> {
    let mut source = std::env::current_dir()?;
    let mut mode = Mode::Menu;
    let mut source_seen = false;
    let mut plain = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        if matches!(
            argument,
            "install"
                | "update"
                | "--update"
                | "--setup-cli"
                | "--setup-oomd"
                | "--install-stealth"
                | "--phase"
        ) && mode != Mode::Menu
        {
            return Err(error("Choose one installer operation"));
        }
        match argument {
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
            "install" => mode = Mode::Install,
            "update" | "--update" => mode = Mode::Update,
            "--setup-cli" => mode = Mode::SetupCli,
            "--plain" => plain = true,
            "--setup-oomd" => {
                mode = Mode::Oomd;
                index += 1;
                source = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| error("Missing OOM source path"))?,
                );
            }
            "--install-stealth" => {
                index += 1;
                let payload = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| error("Missing Stealth payload path"))?,
                );
                mode = Mode::Stealth(payload);
            }
            "--phase" => {
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
    Ok(Options {
        source,
        source_seen,
        mode,
        plain,
    })
}
fn entry() -> Result<()> {
    let mut args = std::env::args();
    let launched_as_hype = args
        .next()
        .is_some_and(|name| std::path::Path::new(&name).file_name() == Some("hype".as_ref()));
    let options = parse(&args.collect::<Vec<_>>())?;
    if options.mode == Mode::Help {
        println!(
            "Usage: hype install [--source REPOSITORY] [--plain]\n       hype update [--plain]\n       hype-install [--source REPOSITORY] [--phase NAME] [--plain]\n       hype-install --setup-cli\n       hype-install --setup-oomd REPOSITORY\n       sudo hype-install --install-stealth PAYLOAD\n\ninstall    Open the installer checklist\nupdate     Upgrade Arch and AUR packages with an optional desktop refresh\n--plain    Use line prompts\n\nPhases: packages user-dirs desktop-integrations lazyvim tray-icons docker-socket backup configs gtk shell system logind firewall warp stealth validate\nRun interactively as a regular Arch Linux user with sudo. The installer sets up yay when it is missing."
        );
        return Ok(());
    }
    let uid = unsafe { libc::geteuid() };
    if matches!(
        options.mode,
        Mode::Menu | Mode::Install | Mode::Update | Mode::SetupCli | Mode::Phase(_)
    ) && uid == 0
    {
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
    if !matches!(options.mode, Mode::Stealth(_) | Mode::SetupCli)
        && unsafe { libc::isatty(libc::STDIN_FILENO) } != 1
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
        optional_groups: None,
        runner: Box::new(ProcessRunner),
    };
    let bootstrap_source = std::env::var_os("HYPE_NIRI_BOOTSTRAP_SOURCE");
    if options.mode == Mode::Install
        && options.source_seen
        && bootstrap_source
            .as_deref()
            .is_some_and(|source| source == "checkout" || source == "main")
    {
        let _lock = InstallLock::acquire(&runtime_directory()?)?;
        let launcher = installer.publish_cli(&std::env::current_exe()?)?;
        println!("Installed {}", launcher.display());
    }
    match options.mode {
        Mode::Menu | Mode::Install | Mode::Update => {
            let direct = match options.mode {
                Mode::Install => Some(tui::Action::Install),
                Mode::Update => Some(tui::Action::Update),
                _ => None,
            };
            let local_source = options.source_seen || (direct.is_none() && !launched_as_hype);
            let fetched =
                options.source_seen && bootstrap_source.as_deref() == Some("main".as_ref());
            if !options.plain && tui::available() {
                return tui::run(installer, direct, local_source, fetched);
            }
            let action = if direct == Some(tui::Action::Update)
                && !installer.confirm("Also refresh Hype Niri desktop files?")?
            {
                tui::Action::Packages
            } else {
                direct.unwrap_or(tui::Action::Install)
            };
            let _source_lock = if action.uses_managed_source(local_source, fetched) {
                Some(installer.prepare_managed_source(!fetched)?)
            } else {
                None
            };
            let _lock = InstallLock::acquire(&runtime_directory()?)?;
            if action == tui::Action::Packages {
                installer.run_plain_package_update()
            } else if action == tui::Action::Update {
                let groups = installer.optional_package_groups()?;
                installer.optional_groups = Some(tui::update_defaults(&installer, &groups)?);
                installer.run_plain_update()
            } else {
                installer.run_all()
            }
        }
        Mode::SetupCli => {
            let _lock = InstallLock::acquire(&runtime_directory()?)?;
            let launcher = installer.publish_cli(&std::env::current_exe()?)?;
            println!("Installed {}", launcher.display());
            Ok(())
        }
        Mode::Oomd => installer.setup_oomd(),
        Mode::Stealth(payload) => installer.install_stealth(&payload),
        Mode::Phase(name) => {
            let _lock = InstallLock::acquire(&runtime_directory()?)?;
            if name != "packages" {
                let mut groups = installer.saved_optional_groups()?.unwrap_or_default();
                match name.as_str() {
                    "lazyvim" => groups.push("editor-neovim".into()),
                    "warp" => groups.push("warp".into()),
                    "stealth" => groups.push("stealth".into()),
                    "firewall" => groups.push("firewall".into()),
                    _ => {}
                }
                installer.optional_groups = Some(groups);
            }
            installer.run_phase(&name)
        }
        Mode::Help => unreachable!(),
    }
}
fn main() {
    if let Err(cause) = entry() {
        let _ = writeln!(std::io::stderr(), "hype-install: {cause}");
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
        assert_eq!(parsed.mode, Mode::Menu);
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

    #[test]
    fn plain_update_is_exclusive_with_manual_phases() {
        let parsed = parse(&arguments(&["--update", "--plain"])).unwrap();
        assert_eq!(parsed.mode, Mode::Update);
        assert!(parsed.plain);
        assert!(parse(&arguments(&["--update", "--phase", "packages"])).is_err());
    }

    #[test]
    fn named_subcommands_select_their_workflows() {
        assert_eq!(parse(&arguments(&["install"])).unwrap().mode, Mode::Install);
        assert_eq!(parse(&arguments(&["update"])).unwrap().mode, Mode::Update);
        assert!(parse(&arguments(&["install", "update"])).is_err());
    }
}
