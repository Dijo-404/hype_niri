use crate::{Installer, Result, error, terminal_color};
use std::os::unix::fs::DirBuilderExt;
use std::{
    fs,
    io::{self, IsTerminal},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    time::Duration,
};

pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

pub type Phase = fn(&mut Installer) -> Result<()>;
pub const PACKAGE_PHASES: &[(&str, Phase)] = &[
    ("Upgrade Arch packages", Installer::update_system_packages),
    ("Upgrade AUR packages", Installer::update_aur_packages),
];
pub const PHASES: &[(&str, Phase)] = &[
    ("Preflight checks", Installer::preflight),
    (
        "Optional package selection",
        Installer::select_optional_groups,
    ),
    ("Mirror refresh", Installer::refresh_mirrors),
    PACKAGE_PHASES[0],
    PACKAGE_PHASES[1],
    ("Package installation", Installer::install_packages),
    ("Configuration backups", Installer::backup_configs),
    ("Configuration publication", Installer::copy_configs),
    ("LazyVim", Installer::setup_lazyvim),
    ("Zsh", Installer::setup_shell),
    ("GTK theme", Installer::setup_gtk),
    ("Desktop integration", Installer::setup_desktop_integrations),
    ("System services", Installer::setup_system),
    ("Lid behavior", Installer::setup_logind),
    ("Firewall", Installer::setup_firewall),
    ("Cloudflare WARP", Installer::setup_cloudflare),
    ("Stealth routing", Installer::setup_stealth),
    ("Validation", Installer::validate),
];

fn heading(title: &str, phase: Option<(usize, usize)>) {
    let (accent, reset) = if terminal_color(io::stdout().is_terminal()) {
        ("\x1b[1;36m", "\x1b[0m")
    } else {
        ("", "")
    };
    if let Some((current, total)) = phase {
        println!("\n{accent}[{current:>2}/{total}]{reset} {title}");
    } else {
        println!("\n{accent}{title}{reset}");
    }
}

pub struct InstallLock {
    _file: fs::File,
}
impl InstallLock {
    pub fn acquire(directory: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(directory)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(error(
                "Installer runtime directory is not an owned real directory",
            ));
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("hype-niri-install.lock"))?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(error("Installer lock is not an owned regular file"));
        }
        Self::lock(file, "Another Hype Niri installation is already running")
    }

    pub(crate) fn lock_source(directory: &Path) -> Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(directory)?;
        Self::lock(
            file,
            "Another Hype Niri operation is using the downloaded checkout",
        )
    }

    fn lock(file: fs::File, busy: &str) -> Result<Self> {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error(busy));
        }
        Ok(Self { _file: file })
    }
}
pub fn runtime_directory() -> Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let hint = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    if fs::symlink_metadata(&hint)
        .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.permissions().mode() & 0o200 != 0)
    {
        return Ok(hint);
    }
    let fallback = PathBuf::from(format!("/tmp/hype-niri-{uid}"));
    match fs::DirBuilder::new().mode(0o700).create(&fallback) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(&fallback)?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(error("Unsafe installer runtime directory"));
    }
    fs::set_permissions(&fallback, fs::Permissions::from_mode(0o700))?;
    Ok(fallback)
}

impl Installer {
    pub fn preflight(&mut self) -> Result<()> {
        if !self.runner.available("pacman") {
            return Err(error("This installer requires Arch Linux (pacman)"));
        }
        for dir in [
            "niri",
            "waybar",
            "scripts",
            "alacritty",
            "mako",
            "fastfetch",
            "hypr",
            "native",
            "notch",
            "installer",
        ] {
            if !self.source.join(dir).is_dir() {
                return Err(error(format!("Required source directory missing: {dir}")));
            }
        }
        for file in [
            "Cargo.toml",
            "Cargo.lock",
            "blueman/Cargo.toml",
            "blueman/src/lib.rs",
            "pkglist.txt",
            "optional-packages.txt",
            "zsh/.zshrc",
            "zsh/.p10k.zsh",
            "niri/config.kdl",
            "niri/opacity.kdl",
            "waybar/config.jsonc",
            "waybar/style.css",
            "scripts/notch.css",
            "gtk/gtk.css",
            "fontconfig/60-hype-niri-fonts.conf",
            "systemd/user/hype-auto-power-profile.service",
            "systemd/user/hype-sleep-watch.service",
            "systemd/user/hype-clipboard-watch.service",
            "systemd/user@.service.d/60-hype-niri-oomd.conf",
            "wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf",
            "polkit/49-udisks2.rules",
            "polkit/50-network-manager.rules",
            "native/Cargo.toml",
            "installer/Cargo.toml",
        ] {
            fs::File::open(self.source.join(file))
                .map_err(|e| error(format!("Required source file {file}: {e}")))?;
        }
        for icon in [
            "blueman-tray",
            "blueman-disabled",
            "blueman-active",
            "nm-signal-00",
            "nm-signal-25",
            "nm-signal-50",
            "nm-signal-75",
            "nm-signal-100",
            "nm-no-connection",
        ] {
            let asset = format!("waybar/icons/16x16/panel/{icon}.svg");
            fs::File::open(self.source.join(&asset))
                .map_err(|cause| error(format!("Required source asset {asset}: {cause}")))?;
        }
        if self.runner.available("curl") {
            if !self
                .output(
                    "curl",
                    &[
                        "--connect-timeout",
                        "5",
                        "--max-time",
                        "15",
                        "-fsS",
                        "https://archlinux.org",
                    ],
                )?
                .success()
            {
                return Err(error("Network preflight failed"));
            }
        } else if self.runner.available("wget") {
            if !self
                .output(
                    "wget",
                    &["--timeout=5", "--spider", "-q", "https://archlinux.org"],
                )?
                .success()
            {
                return Err(error("Network preflight failed"));
            }
        } else {
            self.warn(format_args!(
                "Network preflight unavailable; package operations will report network failures"
            ));
        }
        Ok(())
    }
    pub fn refresh_mirrors(&mut self) -> Result<()> {
        if !self.confirm("Refresh Arch mirrors before installing packages?")? {
            return Ok(());
        }
        let destination = self.system("/etc/pacman.d/mirrorlist");
        if destination.exists() {
            self.write_system(
                "/etc/pacman.d/mirrorlist.hype-niri.bak",
                &fs::read(&destination)?,
                0o644,
            )?;
        }
        let stage = tempfile::NamedTempFile::new()?;
        let mut content = String::new();
        if self.runner.available("reflector")
            && self
                .command(
                    "reflector",
                    &[
                        "--protocol",
                        "https",
                        "--latest",
                        "30",
                        "--sort",
                        "rate",
                        "--save",
                        stage
                            .path()
                            .to_str()
                            .ok_or_else(|| error("Invalid temporary mirror path"))?,
                    ],
                )
                .is_ok()
        {
            content = fs::read_to_string(stage.path())?;
            if !content
                .lines()
                .any(|line| line.starts_with("Server = https://"))
            {
                content.clear();
            }
        }
        if content.is_empty() {
            let url = "https://archlinux.org/mirrorlist/?country=all&protocol=https&ip_version=4&use_mirror_status=on";
            let reply = if self.runner.available("curl") {
                self.output(
                    "curl",
                    &["--connect-timeout", "10", "--max-time", "60", "-fsSL", url],
                )?
            } else {
                self.output("wget", &["--timeout=10", "-qO-", url])?
            };
            if !reply.success() {
                return Err(error(
                    "Mirror download failed; existing mirrorlist retained",
                ));
            }
            if !reply
                .stdout
                .lines()
                .any(|line| line.starts_with("#Server = https://"))
            {
                return Err(error(
                    "Downloaded mirrorlist contains no HTTPS servers; existing mirrors retained",
                ));
            }
            content = reply
                .stdout
                .lines()
                .map(|line| {
                    line.strip_prefix("#Server = https:")
                        .map(|rest| format!("Server = https:{rest}"))
                        .unwrap_or_else(|| line.to_owned())
                })
                .collect::<Vec<_>>()
                .join("\n");
            content.push('\n');
        }
        self.write_system("/etc/pacman.d/mirrorlist", content.as_bytes(), 0o644)
    }
    fn update_system_packages(&mut self) -> Result<()> {
        if !self.runner.available("pacman") {
            return Err(error("This installer requires Arch Linux (pacman)"));
        }
        let log = tempfile::Builder::new()
            .prefix("hype-niri-upgrade-")
            .suffix(".log")
            .tempfile()?;
        let result = self
            .runner
            .run_logged(
                "sudo",
                &["pacman", "-Sy", "--needed", "archlinux-keyring"],
                log.path(),
            )
            .and_then(|()| {
                self.runner
                    .run_logged("sudo", &["pacman", "-Syu"], log.path())
            });
        if let Err(failure) = result {
            let (_, path) = log.keep().map_err(|e| e.error)?;
            return Err(error(format!(
                "System upgrade failed: {failure}. Full log: {}",
                path.display()
            )));
        }
        Ok(())
    }
    pub fn run_all(&mut self) -> Result<()> {
        self.run_plain("Installation", false)
    }

    pub fn run_phase(&mut self, name: &str) -> Result<()> {
        if matches!(name, "configs" | "gtk" | "shell") {
            self.backup_configs()?;
        }
        match name {
            "packages" => {
                self.install_packages()?;
                self.save_optional_groups()
            }
            "user-dirs" => self.setup_user_dirs(),
            "desktop-integrations" => self.setup_desktop_integrations(),
            "lazyvim" => self.setup_lazyvim(),
            "tray-icons" => self.setup_tray_icons(),
            "docker-socket" => self.configure_docker_socket(),
            "backup" => self.backup_configs(),
            "configs" => self.copy_configs(),
            "gtk" => self.setup_gtk(),
            "shell" => self.setup_shell(),
            "system" => self.setup_system(),
            "logind" => self.setup_logind(),
            "firewall" => self.setup_firewall(),
            "warp" => self.setup_cloudflare(),
            "stealth" => self.setup_stealth(),
            "validate" => self.validate(),
            _ => Err(error(format!("Unknown phase: {name}"))),
        }
    }

    pub fn run_plain_update(&mut self) -> Result<()> {
        self.run_plain("Update", true)
    }

    fn run_plain(&mut self, operation: &str, refresh_cli: bool) -> Result<()> {
        heading("Hype Niri · Arch Linux desktop setup", None);
        println!("  Upgrade Arch and AUR packages, then apply the desktop and selected extras.");
        println!("  Install yay from the AUR if it is missing.");
        println!("  Existing desktop configs require a backup before replacement.\n");
        if !self.confirm(&format!("Start {}?", operation.to_lowercase()))? {
            println!("{operation} cancelled");
            return Ok(());
        }
        self.run_plain_phases(PHASES)?;
        if refresh_cli {
            self.refresh_cli()?;
        }
        heading(&format!("{operation} complete"), None);
        if let Some(backup) = &self.backup {
            println!("  Configuration backup: {}", backup.display());
        }
        println!("  Reboot, then select niri-session.");
        println!("  Super+A opens the launcher. Super+T opens a terminal.");
        if self.selected("hype-flow") {
            println!("  Alt+Space starts or finishes dictation with the default shortcut.");
        }
        Ok(())
    }

    pub fn run_plain_package_update(&mut self) -> Result<()> {
        heading("Update system packages", None);
        println!("  Upgrade Arch and AUR packages. Install yay from the AUR if it is missing.");
        if !self.confirm("Start the system package update?")? {
            println!("Package update cancelled");
            return Ok(());
        }
        self.run_plain_phases(PACKAGE_PHASES)?;
        println!("Package update complete. Restart when ready if system components were upgraded.");
        Ok(())
    }

    fn run_plain_phases(&mut self, phases: &[(&str, Phase)]) -> Result<()> {
        self.run_phases(phases, |index, name| {
            heading(name, Some((index + 1, phases.len())));
            Ok(())
        })
    }

    pub fn run_phases(
        &mut self,
        phases: &[(&str, Phase)],
        mut start: impl FnMut(usize, &str) -> Result<()>,
    ) -> Result<()> {
        for (index, (name, run)) in phases.iter().enumerate() {
            start(index, name)?;
            run(self).map_err(|e| error(format!("Phase '{name}' failed: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, Runner};
    use std::{cell::RefCell, rc::Rc};
    struct Fake {
        yes: bool,
        calls: Rc<RefCell<Vec<String>>>,
    }
    impl Runner for Fake {
        fn output(&self, p: &str, a: &[&str]) -> Result<CommandOutput> {
            self.calls.borrow_mut().push(format!("{p} {}", a.join(" ")));
            Ok(CommandOutput {
                code: 1,
                stdout: "invalid mirrors".into(),
                stderr: String::new(),
            })
        }
        fn run(&self, p: &str, a: &[&str]) -> Result<()> {
            self.calls.borrow_mut().push(format!("{p} {}", a.join(" ")));
            Ok(())
        }
        fn confirm(&self, _: &str) -> Result<bool> {
            Ok(self.yes)
        }
        fn input(&self, _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn available(&self, p: &str) -> bool {
            p != "reflector"
        }
    }
    fn fixture(yes: bool) -> (tempfile::TempDir, Installer, Rc<RefCell<Vec<String>>>) {
        let root = tempfile::tempdir().unwrap();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let installer = Installer {
            source: root.path().join("source"),
            home: root.path().join("home"),
            root: root.path().join("system"),
            backup: None,
            build_dir: None,
            optional_groups: None,
            runner: Box::new(Fake {
                yes,
                calls: calls.clone(),
            }),
        };
        (root, installer, calls)
    }
    #[test]
    fn declined_upgrade_never_syncs_package_databases() {
        let (_root, mut i, calls) = fixture(false);
        i.run_plain_package_update().unwrap();
        assert!(calls.borrow().is_empty());
    }
    #[test]
    fn declined_mirror_refresh_leaves_files_and_commands_alone() {
        let (_root, mut i, calls) = fixture(false);
        i.refresh_mirrors().unwrap();
        assert!(calls.borrow().is_empty());
    }
    #[test]
    fn invalid_mirror_download_preserves_original() {
        let (_root, mut i, _) = fixture(true);
        i.write_system("/etc/pacman.d/mirrorlist", b"original\n", 0o644)
            .unwrap();
        assert!(i.refresh_mirrors().is_err());
        assert_eq!(
            fs::read(i.system("/etc/pacman.d/mirrorlist")).unwrap(),
            b"original\n"
        );
    }
    #[test]
    fn concurrent_installer_is_rejected_until_holder_exits() {
        let root = tempfile::tempdir().unwrap();
        let first = InstallLock::acquire(root.path()).unwrap();
        assert!(InstallLock::acquire(root.path()).is_err());
        drop(first);
        assert!(InstallLock::acquire(root.path()).is_ok());
    }
    #[test]
    fn symlink_lock_never_truncates_external_file() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("external");
        fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("hype-niri-install.lock")).unwrap();
        assert!(InstallLock::acquire(root.path()).is_err());
        assert_eq!(fs::read(target).unwrap(), b"keep");
    }
    #[test]
    fn missing_source_stops_before_network_requests() {
        let (_root, mut i, calls) = fixture(true);
        assert!(i.preflight().is_err());
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn missing_tray_asset_stops_before_network_or_package_mutation() {
        let (_root, mut installer, calls) = fixture(true);
        fs::create_dir_all(&installer.source).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for entry in fs::read_dir(repo).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name == "waybar" || name == "target" || name == ".git" {
                continue;
            }
            std::os::unix::fs::symlink(entry.path(), installer.source.join(name)).unwrap();
        }
        let panel = installer.source.join("waybar/icons/16x16/panel");
        fs::create_dir_all(&panel).unwrap();
        for name in ["config.jsonc", "style.css"] {
            fs::copy(
                repo.join("waybar").join(name),
                installer.source.join("waybar").join(name),
            )
            .unwrap();
        }
        for name in [
            "blueman-tray",
            "blueman-disabled",
            "blueman-active",
            "nm-signal-00",
            "nm-signal-25",
            "nm-signal-50",
            "nm-signal-75",
            "nm-no-connection",
        ] {
            fs::write(panel.join(format!("{name}.svg")), b"fixture").unwrap();
        }
        let failure = installer.preflight().unwrap_err().to_string();
        assert!(failure.contains("nm-signal-100.svg"), "{failure}");
        assert!(calls.borrow().is_empty());
    }
    #[test]
    fn failed_phase_stops_before_package_or_reboot_commands() {
        let (_root, mut i, calls) = fixture(true);
        assert!(i.run_all().unwrap_err().to_string().contains("Preflight"));
        assert!(calls.borrow().is_empty());
    }
}
