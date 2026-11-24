use crate::{Installer, Result, error};
use std::{
    ffi::CString,
    fs::{self, File, FileTimes, Metadata, Permissions},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tempfile::{Builder, NamedTempFile};

pub(crate) const DESKTOP_PROGRAMS: &[&str] = &["hype-notch", "hype-control", "hype-status"];

const DESKTOP_DIRS: &[&str] = &[
    "scripts",
    "niri",
    "waybar",
    "alacritty",
    "mako",
    "fastfetch",
    "hypr",
];
const LEGACY_CONTROLS: &[&str] = &[
    "temperature-control",
    "brightness-control",
    "volume-control",
    "mic-control",
    "power-profile",
    "caffeine-control",
    "display-scale",
    "monitor-refresh",
    "opacity-toggle",
    "hotspot",
    "open-drives",
    "copilot-shortcut",
    "copilot-display",
    "prepare-sleep",
    "suspend-now",
    "lock-dim",
    "wallpaper",
    "lock",
    "lock-screen",
    "fullscreen-toggle",
    "menu-toggle",
    "password-prompt",
    "power-menu",
    "start-tray-applets",
    "start-waybar",
    "stealth-indicator",
    "warp-indicator",
];
const XDG_USER_DIRS: &[(&str, &str, &str)] = &[
    ("DESKTOP", "desktop", "Desktop"),
    ("DOWNLOAD", "downloads", "Downloads"),
    ("TEMPLATES", "templates", "Templates"),
    ("PUBLICSHARE", "public", "Public"),
    ("DOCUMENTS", "documents", "Documents"),
    ("MUSIC", "music", "Music"),
    ("PICTURES", "pictures", "Pictures"),
    ("VIDEOS", "videos", "Videos"),
    ("PROJECTS", "projects", "Projects"),
];
const BRAVE_DEFAULTS: &[&str] = &[
    "text/html",
    "application/xhtml+xml",
    "x-scheme-handler/http",
    "x-scheme-handler/https",
    "x-scheme-handler/about",
    "x-scheme-handler/unknown",
];
const GTK_SETTINGS: &[u8] = b"[Settings]\n\
gtk-theme-name=Adwaita-dark\n\
gtk-icon-theme-name=Papirus-Dark\n\
gtk-cursor-theme-name=Bibata-Modern-Ice\n\
gtk-cursor-theme-size=24\n\
gtk-font-name=JetBrains Mono 10\n\
gtk-application-prefer-dark-theme=true\n";

// Old user units execute this through Bash until their replacement has been reloaded.
const AUTO_POWER_TRANSITION: &[u8] =
    b"#!/usr/bin/env bash\nexec \"${BASH_SOURCE[0]%/*}/hype-control\" auto-power-profile \"$@\"\n";
// A running bar or custom config can retain old command paths after disk configuration changes.
const WAYBAR_COMPATIBILITY: &[&str] = &[
    "menu-toggle",
    "power-menu",
    "warp-indicator",
    "stealth-indicator",
];

impl Installer {
    pub fn backup_configs(&mut self) -> Result<()> {
        let targets = [
            ".zshrc",
            ".p10k.zsh",
            ".config/hype-niri",
            ".config/gtk-3.0",
            ".config/gtk-4.0",
            ".config/autostart",
            ".config/fontconfig",
            ".config/systemd/user",
            ".config/wireplumber",
            ".config/mimeapps.list",
            ".local/share/applications/blueman-manager.desktop",
            ".local/share/icons/Papirus-Dark",
            ".local/share/stealth",
            ".local/share/privacy-shield",
        ]
        .into_iter()
        .map(PathBuf::from)
        .chain(
            DESKTOP_DIRS
                .iter()
                .map(|dir| Path::new(".config").join(dir)),
        );
        let mut existing = Vec::new();
        for target in targets {
            if metadata(&self.home.join(&target))?.is_some() {
                existing.push(target);
            }
        }
        if existing.is_empty() {
            println!("No existing configs to back up");
            return Ok(());
        }
        if !self.confirm("Back up existing configs before replacing them?")? {
            return Err(error("Backup declined. Existing configs were retained."));
        }

        let backup = self.backup_directory()?;
        for target in existing {
            let destination = backup.join(&target);
            fs::create_dir_all(destination.parent().unwrap())?;
            copy_entry(&self.home.join(&target), &destination, false)
                .map_err(|err| error(format!("Could not back up {}: {err}", target.display())))?;
        }
        println!("Backup saved to {}", backup.display());
        Ok(())
    }

    fn backup_directory(&mut self) -> Result<PathBuf> {
        if let Some(backup) = &self.backup {
            return Ok(backup.clone());
        }

        let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let backup = Builder::new()
            .prefix(&format!(".config-backup-{seconds}."))
            .tempdir_in(&self.home)?
            .keep();
        self.backup = Some(backup.clone());
        Ok(backup)
    }

    pub fn build_programs(&mut self) -> Result<PathBuf> {
        if let Some(build) = &self.build_dir {
            return Ok(build.path().to_owned());
        }
        println!("Building native Rust desktop programs");
        let build = self.build_rust(
            &[
                "--package",
                "hype-notch",
                "--bin",
                "hype-notch",
                "--package",
                "hype-desktop",
                "--bin",
                "hype-control",
                "--bin",
                "hype-status",
            ],
            DESKTOP_PROGRAMS,
            "--check",
        )?;
        let release = build.path().to_owned();
        self.build_dir = Some(build);
        Ok(release)
    }

    pub(crate) fn build_stealth(&self) -> Result<tempfile::TempDir> {
        self.build_rust(
            &["--package", "hype-desktop", "--bin", "stealth"],
            &["stealth"],
            "version",
        )
    }

    fn build_rust(
        &self,
        targets: &[&str],
        binaries: &[&str],
        check: &str,
    ) -> Result<tempfile::TempDir> {
        let target = self.prepare_user_directory(".cache/hype-niri/cargo-target")?;
        let metadata = fs::symlink_metadata(&target)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(error("Unsafe Cargo build cache directory"));
        }
        let lease = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(target.join("hype-install.lock"))?;
        if !lease.metadata()?.is_file() {
            return Err(error("Unsafe Cargo build cache lock"));
        }
        lease.lock()?;
        let host = self.cargo_host()?;
        let manifest = self.source.join("Cargo.toml");
        // An inherited build.target must not redirect the build away from the binaries we publish.
        let mut args = vec!["build", "--release", "--locked", "--target", &host];
        args.extend_from_slice(targets);
        args.extend([
            "--manifest-path",
            path_arg(&manifest)?,
            "--target-dir",
            path_arg(&target)?,
        ]);
        self.command("cargo", &args).map_err(|err| {
            error(format!(
                "Could not build native desktop programs: {err}. Check rust, gtk4, gtk4-layer-shell and libpulse."
            ))
        })?;
        let release = target.join(host).join("release");
        let staged = Builder::new().prefix("hype-niri-binaries.").tempdir()?;
        // Publish from a private snapshot in case another install reuses the cache.
        for name in binaries {
            let binary = release.join(name);
            self.validate_binary(&binary, check)?;
            copy_regular(&binary, &staged.path().join(name), Some(0o755), false)?;
        }
        Ok(staged)
    }

    pub(crate) fn cargo_host(&self) -> Result<String> {
        let cargo = self.output("cargo", &["-vV"])?;
        cargo
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .filter(|host| {
                cargo.success()
                    && !host.is_empty()
                    && host
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
            })
            .map(str::to_owned)
            .ok_or_else(|| error("Could not determine Cargo's host target"))
    }

    fn validate_binary(&self, path: &Path, check: &str) -> Result<()> {
        let runnable = fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
        if !runnable {
            return Err(error(format!(
                "Built program {} is missing or not executable",
                path.display()
            )));
        }
        self.command(path_arg(path)?, &[check]).map_err(|err| {
            error(format!(
                "Built program {} failed validation: {err}",
                path.display()
            ))
        })
    }

    pub fn copy_configs(&mut self) -> Result<()> {
        let release = self.build_programs()?;
        let config = self.prepare_user_directory(".config")?;
        for dir in DESKTOP_DIRS {
            let stage = Builder::new()
                .prefix(&format!(".{dir}."))
                .tempdir_in(&config)?;
            let staged = stage.path().join(dir);
            fs::create_dir(&staged)?;
            copy_contents(&self.source.join(dir), &staged, false)
                .map_err(|err| error(format!("Could not stage {dir}: {err}")))?;

            match *dir {
                "scripts" => {
                    for binary in DESKTOP_PROGRAMS {
                        copy_regular(
                            &release.join(binary),
                            &staged.join(binary),
                            Some(0o755),
                            true,
                        )?;
                    }
                    // Keep the previous Niri binding usable until its configuration is replaced.
                    for (name, program) in LEGACY_CONTROLS
                        .iter()
                        .copied()
                        .map(|name| (name, "hype-control"))
                        .chain([("notch", "hype-notch")])
                    {
                        let path = staged.join(format!("{name}.sh"));
                        remove_file(&path)?;
                        symlink(program, path)?;
                    }
                    write_file(
                        &staged.join("auto-power-profile.sh"),
                        AUTO_POWER_TRANSITION,
                        0o755,
                    )?;
                }
                "waybar" => {
                    let path = staged.join("config.jsonc");
                    let contents = fs::read_to_string(&path)?;
                    let mut modules = vec!["\"custom/hotspot\"", "\"tray\""];
                    if self.selected("warp") {
                        modules.push("\"custom/warp\"");
                    }
                    if self.selected("stealth") {
                        modules.push("\"image#stealth\"");
                    }
                    let default = "\"modules\": [\"custom/hotspot\", \"tray\"]";
                    if !contents.contains(default) {
                        return Err(error("Waybar connectivity group template is missing"));
                    }
                    write_file(
                        &path,
                        contents
                            .replace(default, &format!("\"modules\": [{}]", modules.join(", ")))
                            .as_bytes(),
                        0o644,
                    )?;
                }
                "niri" => {
                    if self.selected("theme-extras") {
                        let path = staged.join("config.kdl");
                        let contents = fs::read_to_string(&path)?;
                        write_file(
                            &path,
                            contents
                                .replace(
                                    "xcursor-theme \"Adwaita\"",
                                    "xcursor-theme \"Bibata-Modern-Ice\"",
                                )
                                .as_bytes(),
                            0o644,
                        )?;
                    }
                    let saved = config.join("niri/outputs.kdl");
                    match fs::metadata(&saved) {
                        Ok(metadata) if metadata.is_file() => {
                            copy_regular(&saved, &staged.join("outputs.kdl"), None, true).map_err(
                                |err| {
                                    error(format!(
                                        "Could not preserve saved monitor settings: {err}"
                                    ))
                                },
                            )?;
                        }
                        Ok(_) => {}
                        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                        Err(err) => return Err(err.into()),
                    }
                }
                _ => {}
            }
            replace_directory(&staged, &config.join(dir))?;
            println!("Copied {dir} to ~/.config/{dir}");
        }

        for name in LEGACY_CONTROLS.iter().copied().chain(["notch"]) {
            if WAYBAR_COMPATIBILITY.contains(&name) {
                continue;
            }
            remove_legacy_link(&config.join(format!("scripts/{name}.sh")))?;
        }
        let control = config.join("scripts/hype-control");
        let _ = self.output(path_arg(&control)?, &["display-scale", "--no-reload"]);
        let outputs = config.join("niri/outputs.kdl");
        if metadata(&outputs)?.is_none() {
            write_file(&outputs, b"", 0o644)?;
        }

        self.setup_user_dirs()?;
        if fs::symlink_metadata(self.user("Drives")).is_ok_and(|metadata| metadata.is_dir()) {
            adopt_lowercase_dir(&self.user("Drives"), &self.user("drives"))?;
        }
        let pictures = self.prepare_user_directory("pictures")?;
        adopt_lowercase_dir(&pictures.join("Screenshots"), &pictures.join("screenshots"))?;
        adopt_lowercase_dir(&pictures.join("Wallpapers"), &pictures.join("wallpapers"))?;
        let wallpapers = self.prepare_user_directory("pictures/wallpapers")?;
        let source = self.source.join("wallpapers");
        if source.is_dir() {
            copy_contents(&source, &wallpapers, true)?;
        } else {
            eprintln!("No wallpapers found in source directory");
        }

        let pointer = self
            .prepare_user_directory(".local/state/niri")?
            .join("current_wallpaper");
        if !pointer.exists() {
            if let Some(wallpaper) = first_wallpaper(&wallpapers)? {
                write_symlink(&wallpaper, &pointer)?;
            } else {
                eprintln!("No wallpaper available to seed current_wallpaper");
            }
        }
        self.save_optional_groups()?;
        let autostart = self.prepare_user_directory(".config/autostart")?;
        write_file(
            &autostart.join("blueman.desktop"),
            b"[Desktop Entry]\nType=Application\nHidden=true\n",
            0o644,
        )?;
        Ok(())
    }

    pub fn save_optional_groups(&self) -> Result<()> {
        let Some(groups) = &self.optional_groups else {
            return Ok(());
        };
        let directory = self.prepare_user_directory(".config/hype-niri")?;
        let mut contents = groups.join("\n");
        if !contents.is_empty() {
            contents.push('\n');
        }
        write_file(
            &directory.join("optional-groups"),
            contents.as_bytes(),
            0o600,
        )?;
        Ok(())
    }

    pub fn setup_user_dirs(&mut self) -> Result<()> {
        let can_read = self.runner.available("xdg-user-dir");
        let can_set = self.runner.available("xdg-user-dirs-update");
        if can_set {
            self.prepare_user_directory(".config")?;
        }
        for (name, dir, legacy) in XDG_USER_DIRS {
            let target = self.user(dir);
            if can_read
                && let Ok(output) = self.output("xdg-user-dir", &[name])
                && output.success()
            {
                let configured =
                    Path::new(output.stdout.trim_end_matches('\n').trim_end_matches('/'));
                if configured.parent() == Some(self.home.as_path()) {
                    adopt_lowercase_dir(configured, &target)?;
                }
            }

            adopt_lowercase_dir(&self.user(legacy), &target)?;
            if can_set {
                self.command("xdg-user-dirs-update", &["--set", name, path_arg(&target)?])?;
            }
        }
        if !can_set {
            eprintln!("xdg-user-dirs-update not found; created lowercase directories");
        }
        Ok(())
    }

    pub fn setup_lazyvim(&mut self) -> Result<()> {
        if !self.selected("editor-neovim") {
            return Ok(());
        }
        let config = self.user(".config/nvim");
        if fs::read_to_string(config.join("lua/config/lazy.lua"))
            .is_ok_and(|contents| contents.contains("LazyVim/LazyVim"))
        {
            println!("Existing LazyVim configuration retained");
            return Ok(());
        }

        let parent = self.prepare_user_directory(".config")?;
        let stage = Builder::new().prefix(".nvim.").tempdir_in(parent)?;
        let staged = stage.path().join("nvim");
        self.command(
            "git",
            &[
                "clone",
                "--depth",
                "1",
                "https://github.com/LazyVim/starter.git",
                path_arg(&staged)?,
            ],
        )
        .map_err(|err| error(format!("Could not download LazyVim: {err}")))?;
        if !staged.join("init.lua").is_file() || !staged.join("lua/config/lazy.lua").is_file() {
            return Err(error(
                "LazyVim starter is incomplete. Existing Neovim configuration was retained.",
            ));
        }
        remove_entry(&staged.join(".git"))?;

        if metadata(&config)?.is_some() {
            let backup = self.backup_directory()?.join(".config/nvim");
            fs::create_dir_all(backup.parent().unwrap())?;
            copy_entry(&config, &backup, false)?;
            println!("Backed up Neovim configuration to {}", backup.display());
        }
        replace_directory(&staged, &config)?;
        println!("LazyVim installed. Open nvim to download plugins, then run :LazyHealth.");
        Ok(())
    }

    pub fn setup_shell(&mut self) -> Result<()> {
        let plugin = "fzf-tab/fzf-tab.plugin.zsh";
        if self.selected("shell-extras")
            && !self
                .system(&format!("/usr/share/zsh/plugins/{plugin}"))
                .is_file()
            && !self.user(&format!(".zsh/{plugin}")).is_file()
        {
            eprintln!("fzf-tab not found. Check the fzf-tab package from pkglist.txt.");
        }

        for dotfile in [".zshrc", ".p10k.zsh"] {
            copy_regular(
                &self.source.join("zsh").join(dotfile),
                &self.user(dotfile),
                None,
                true,
            )?;
        }
        let shell = std::env::var_os("SHELL").unwrap_or_default();
        if Path::new(&shell)
            .file_name()
            .is_some_and(|name| name == "zsh")
        {
            println!("Zsh is already the default shell");
        } else if self.confirm("Change default shell to zsh?")? {
            self.command("chsh", &["-s", "/usr/bin/zsh"])?;
            println!("Default shell changed to zsh. Log out and back in for this to take effect.");
        }
        Ok(())
    }

    pub fn setup_gtk(&mut self) -> Result<()> {
        let cursor = if self.selected("theme-extras") {
            "Bibata-Modern-Ice"
        } else {
            "Adwaita"
        };
        let settings_contents =
            String::from_utf8_lossy(GTK_SETTINGS).replace("Bibata-Modern-Ice", cursor);
        for dir in [".config/gtk-3.0", ".config/gtk-4.0"] {
            let settings = self.prepare_user_directory(dir)?.join("settings.ini");
            write_file(&settings, settings_contents.as_bytes(), 0o644)?;
        }
        let fonts = self
            .prepare_user_directory(".config/fontconfig/conf.d")?
            .join("60-hype-niri-fonts.conf");
        copy_regular(
            &self.source.join("fontconfig/60-hype-niri-fonts.conf"),
            &fonts,
            Some(0o644),
            true,
        )?;
        if !self.selected("theme-extras") {
            // Core uses the installed Papirus colors.
        } else if !self.runner.available("papirus-folders") {
            eprintln!(
                "papirus-folders not found. Install papirus-folders-catppuccin-git to recolor folders."
            );
        } else if self
            .command(
                "papirus-folders",
                &["-C", "grey", "--theme", "Papirus-Dark"],
            )
            .is_err()
        {
            eprintln!("papirus-folders failed. Folder color was unchanged.");
        }
        self.setup_tray_icons()?;

        let dconf = self.runner.available("dconf");
        if !dconf && !self.runner.available("gsettings") {
            eprintln!(
                "Neither dconf nor gsettings found. GTK settings.ini files will still apply."
            );
            return Ok(());
        }
        for (key, value) in [
            ("color-scheme", "prefer-dark"),
            ("gtk-theme", "Adwaita-dark"),
            ("icon-theme", "Papirus-Dark"),
            ("cursor-theme", cursor),
            ("cursor-size", "24"),
            ("font-name", "JetBrains Mono 10"),
            ("monospace-font-name", "JetBrains Mono 10"),
            ("document-font-name", "JetBrains Mono 10"),
        ] {
            let output = if dconf {
                let value = if key == "cursor-size" {
                    value.to_owned()
                } else {
                    format!("'{value}'")
                };
                self.output(
                    "dconf",
                    &[
                        "write",
                        &format!("/org/gnome/desktop/interface/{key}"),
                        &value,
                    ],
                )
            } else {
                self.output(
                    "gsettings",
                    &["set", "org.gnome.desktop.interface", key, value],
                )
            };
            if !output.is_ok_and(|output| output.success()) {
                eprintln!(
                    "Could not apply all desktop theme settings. GTK settings.ini files will still apply."
                );
                break;
            }
        }
        Ok(())
    }

    pub fn setup_tray_icons(&self) -> Result<()> {
        let theme = self.prepare_user_directory(".local/share/icons/Papirus-Dark")?;
        let index = theme.join("index.theme");
        if !index.is_file() {
            copy_regular(
                &self.system("/usr/share/icons/Papirus-Dark/index.theme"),
                &index,
                None,
                true,
            )?;
        }
        let base = theme.join("16x16");
        prepare_generated_directory(&base)?;
        let double_scale = theme.join("16x16@2x");
        if metadata(&double_scale)?.is_none() {
            symlink("16x16", &double_scale)?;
        }

        for size_dir in [base, double_scale] {
            if fs::read_link(&size_dir).is_ok_and(|target| target == Path::new("16x16")) {
                continue;
            }
            prepare_generated_directory(&size_dir)?;
            let panel = size_dir.join("panel");
            prepare_generated_directory(&panel)?;
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
                let icon = format!("{icon}.svg");
                let source = self.source.join("waybar/icons/16x16/panel").join(&icon);
                copy_regular(&source, &panel.join(&icon), Some(0o644), true)?;
            }
        }
        if self.runner.available("gtk-update-icon-cache")
            && self
                .command("gtk-update-icon-cache", &["-f", "-t", path_arg(&theme)?])
                .is_err()
        {
            eprintln!("Could not refresh the user icon cache");
        }
        Ok(())
    }

    pub fn setup_desktop_integrations(&mut self) -> Result<()> {
        let wants =
            self.prepare_user_directory(".config/systemd/user/graphical-session.target.wants")?;
        self.prepare_user_directory(".config/wireplumber/wireplumber.conf.d")?;
        for file in [
            "systemd/user/hype-auto-power-profile.service",
            "wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf",
        ] {
            copy_regular(
                &self.source.join(file),
                &self.user(".config").join(file),
                Some(0o644),
                true,
            )?;
        }
        write_symlink(
            Path::new("../hype-auto-power-profile.service"),
            &wants.join("hype-auto-power-profile.service"),
        )?;
        let user_manager_running = self.user_manager_running();
        if user_manager_running {
            self.command("systemctl", &["--user", "daemon-reload"])?;
        }
        let scripts = self.user(".config/scripts");
        if fs::symlink_metadata(&scripts).is_ok_and(|metadata| metadata.is_dir()) {
            let transition = scripts.join("auto-power-profile.sh");
            if fs::read(&transition).is_ok_and(|bytes| bytes == AUTO_POWER_TRANSITION) {
                remove_file(&transition)?;
            } else {
                remove_legacy_link(&transition)?;
            }
        }
        if self.selected("bluetooth") {
            self.remove_blueman_wrapper()?;
            if self
                .command(
                    "gsettings",
                    &["set", "org.blueman.general", "notification-daemon", "false"],
                )
                .is_err()
            {
                eprintln!(
                    "Could not enable Bluetooth pairing dialogs. Run gsettings set org.blueman.general notification-daemon false."
                );
            }
        }
        if let Err(err) = self.set_default_browser() {
            eprintln!("Could not set the default browser: {err}");
        }
        self.remove_stale_audio_service()?;
        if let Err(err) = self.configure_file_indexing() {
            eprintln!("Could not configure file indexing exclusions: {err}");
        }
        if user_manager_running {
            self.command("systemctl", &["--user", "daemon-reload"])?;
        }
        self.setup_hype_flow()?;
        Ok(())
    }

    fn user_manager_running(&self) -> bool {
        self.output("systemctl", &["--user", "show-environment"])
            .is_ok_and(|output| output.success())
    }

    fn remove_stale_audio_service(&self) -> Result<()> {
        let unit = self.user(".config/systemd/user/audio-jack-switch.service");
        if !unit.is_file()
            || !fs::read_to_string(&unit)?
                .lines()
                .any(|line| line == "ExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh")
            || self
                .user(".config/waybar/scripts/audio-jack-switch.sh")
                .exists()
        {
            return Ok(());
        }

        self.prepare_user_directory(".config/systemd/user")?;
        let wants = self.prepare_user_directory(".config/systemd/user/default.target.wants")?;
        if self.user_manager_running() {
            self.command(
                "systemctl",
                &["--user", "disable", "--now", "audio-jack-switch.service"],
            )?;
            let _ = self.output(
                "systemctl",
                &["--user", "reset-failed", "audio-jack-switch.service"],
            );
        } else {
            remove_file(&wants.join("audio-jack-switch.service"))?;
        }
        remove_file(&unit)?;
        println!("Removed obsolete audio jack service. WirePlumber handles routing.");
        Ok(())
    }

    fn remove_blueman_wrapper(&self) -> Result<()> {
        let entry = self.user(".local/share/applications/blueman-manager.desktop");
        if entry.is_file()
            && fs::read_to_string(&entry)?.lines().any(|line| {
                line.strip_prefix("Exec=")
                    .is_some_and(|exec| exec.contains("/.config/scripts/blueman-manager.py"))
            })
        {
            self.prepare_user_directory(".local/share/applications")?;
            remove_file(&entry)?;
        }

        let dir = ".config/systemd/user/blueman-manager.service.d";
        let dropin = self.user(dir);
        if dropin.join("hype-niri.conf").symlink_metadata().is_ok() {
            self.prepare_user_directory(dir)?;
            remove_file(&dropin.join("hype-niri.conf"))?;
        }
        if !dropin.is_symlink() {
            let _ = fs::remove_dir(&dropin);
        }
        Ok(())
    }

    fn set_default_browser(&self) -> Result<()> {
        let browser = if self.selected("browser-brave") {
            "brave-browser.desktop"
        } else if self.selected("browser-firefox") {
            "firefox.desktop"
        } else {
            return Ok(());
        };
        let config = self.prepare_user_directory(".config")?;
        let destination = config.join("mimeapps.list");
        let previous = if metadata(&destination)?.is_some() {
            fs::read_to_string(&destination)?
        } else {
            String::new()
        };
        let section = "[Default Applications]";
        let mut updated = String::new();
        let mut blanks = String::new();
        let mut in_defaults = false;
        let mut emitted = false;
        for line in previous.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                blanks.push_str(line);
                blanks.push('\n');
                continue;
            }
            if trimmed.starts_with('[') {
                if in_defaults && !emitted {
                    append_browser_defaults(&mut updated, browser);
                    emitted = true;
                }
                in_defaults = trimmed == section;
            } else if in_defaults
                && trimmed
                    .split_once('=')
                    .is_some_and(|(key, _)| BRAVE_DEFAULTS.contains(&key.trim()))
            {
                continue;
            }
            updated.push_str(&blanks);
            blanks.clear();
            updated.push_str(line);
            updated.push('\n');
        }
        if !emitted {
            if !in_defaults {
                updated.push_str(&blanks);
                blanks.clear();
                updated.push_str(section);
                updated.push('\n');
            }
            append_browser_defaults(&mut updated, browser);
        }
        updated.push_str(&blanks);
        write_file(&destination, updated.as_bytes(), 0o644)?;
        Ok(())
    }

    fn configure_file_indexing(&self) -> Result<()> {
        let schema = "org.freedesktop.Tracker3.Miner.Files";
        let key = "ignored-directories";
        if !self.runner.available("gsettings") {
            return Ok(());
        }
        let Ok(current) = self.output("gsettings", &["get", schema, key]) else {
            return Ok(());
        };
        if !current.success() {
            return Ok(());
        }
        let current = current
            .stdout
            .trim()
            .strip_prefix("@as ")
            .unwrap_or(current.stdout.trim());
        let entries = current
            .strip_prefix('[')
            .and_then(|list| list.strip_suffix(']'))
            .ok_or_else(|| error("Tracker returned an invalid ignored-directories setting"))?;
        let mut updated = entries.trim_end().to_owned();
        for dir in [
            "node_modules",
            ".venv",
            "venv",
            "miniconda3",
            "__pycache__",
            "target",
            ".mypy_cache",
            ".pytest_cache",
            ".ruff_cache",
        ] {
            if !variant_contains(entries, dir) {
                if !updated.trim_start().is_empty() {
                    updated.push_str(", ");
                }
                updated.push('\'');
                updated.push_str(dir);
                updated.push('\'');
            }
        }
        if updated != entries.trim_end() {
            self.command("gsettings", &["set", schema, key, &format!("[{updated}]")])?;
        }
        Ok(())
    }

    fn prepare_user_directory(&self, relative: &str) -> Result<PathBuf> {
        let generated_config = Path::new(relative).starts_with(".config");
        let generated_theme = self.user(".local/share/icons/Papirus-Dark");
        let mut directory = self.home.clone();
        for component in Path::new(relative).components() {
            directory.push(component);
            if generated_config || directory.starts_with(&generated_theme) {
                prepare_generated_directory(&directory)?;
            } else {
                // User data roots can intentionally point at a separate disk.
                fs::create_dir_all(&directory)?;
            }
        }
        Ok(directory)
    }
}

fn metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub(crate) fn path_arg(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        error(format!(
            "Command path is not valid UTF-8: {}",
            path.display()
        ))
    })
}

fn prepare_generated_directory(directory: &Path) -> io::Result<()> {
    if !directory.is_symlink() {
        return fs::create_dir_all(directory);
    }
    let stage = Builder::new()
        .prefix(".hype-config.")
        .tempdir_in(directory.parent().unwrap())?;
    let staged = stage.path().join("directory");
    fs::create_dir(&staged)?;
    match fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => copy_contents(directory, &staged, false)?,
        Ok(_) => {
            return Err(io::Error::other(format!(
                "Expected a configuration directory: {}",
                directory.display()
            )));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    replace_directory(&staged, directory)
}

/// Publish a staged directory in one rename. The caller's temporary directory owns the old entry.
pub(crate) fn replace_directory(staged: &Path, destination: &Path) -> io::Result<()> {
    let swap = if metadata(destination)?.is_some() {
        libc::RENAME_EXCHANGE
    } else {
        libc::RENAME_NOREPLACE
    };
    let original = CString::new(staged.as_os_str().as_bytes())?;
    let replacement = CString::new(destination.as_os_str().as_bytes())?;
    // The C strings stay alive for the duration of the syscall and both paths are resolved from cwd.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            original.as_ptr(),
            libc::AT_FDCWD,
            replacement.as_ptr(),
            swap,
        )
    };
    if result == -1 {
        return Err(io::Error::other(format!(
            "Could not publish {}: {}",
            destination.display(),
            io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn copy_contents(source: &Path, destination: &Path, keep_existing: bool) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        copy_entry(
            &entry.path(),
            &destination.join(entry.file_name()),
            keep_existing,
        )?;
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path, keep_existing: bool) -> io::Result<()> {
    let source_metadata = fs::symlink_metadata(source)?;
    if keep_existing && let Some(existing) = metadata(destination)? {
        if existing.is_dir() && source_metadata.is_dir() {
            copy_contents(source, destination, true)?;
        }
        return Ok(());
    }
    if source_metadata.is_symlink() {
        symlink(fs::read_link(source)?, destination)?;
    } else if source_metadata.is_dir() {
        fs::create_dir(destination)?;
        copy_contents(source, destination, keep_existing)?;
        let directory = File::open(destination)?;
        directory.set_permissions(source_metadata.permissions())?;
        directory.set_times(file_times(&source_metadata))?;
    } else if source_metadata.is_file() {
        copy_regular(source, destination, None, false)?;
    } else {
        return Err(io::Error::other(format!(
            "Unsupported configuration entry: {}",
            source.display()
        )));
    }
    Ok(())
}

fn file_times(metadata: &Metadata) -> FileTimes {
    let mut times = FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    times
}

fn copy_regular(
    source: &Path,
    destination: &Path,
    mode: Option<u32>,
    replace: bool,
) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "Expected a file: {}",
            source.display()
        )));
    }
    let mut incoming = File::open(source)?;
    let mut staged = NamedTempFile::new_in(destination.parent().unwrap())?;
    io::copy(&mut incoming, &mut staged)?;
    let mode = mode
        .map(Permissions::from_mode)
        .unwrap_or_else(|| metadata.permissions());
    staged.as_file().set_permissions(mode)?;
    staged.as_file().set_times(file_times(&metadata))?;
    if replace {
        staged.persist(destination).map_err(|err| err.error)?;
    } else {
        staged
            .persist_noclobber(destination)
            .map_err(|err| err.error)?;
    }
    Ok(())
}

fn write_file(destination: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let mut staged = NamedTempFile::new_in(destination.parent().unwrap())?;
    staged.write_all(contents)?;
    staged
        .as_file()
        .set_permissions(Permissions::from_mode(mode))?;
    staged.persist(destination).map_err(|err| err.error)?;
    Ok(())
}

fn write_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    let stage = Builder::new()
        .prefix(".hype-link.")
        .tempdir_in(destination.parent().unwrap())?;
    let staged = stage.path().join("link");
    symlink(target, &staged)?;
    fs::rename(staged, destination)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_legacy_link(path: &Path) -> io::Result<()> {
    if let Some(metadata) = metadata(path)?
        && metadata.is_symlink()
    {
        remove_file(path)?;
    }
    Ok(())
}

fn remove_entry(path: &Path) -> io::Result<()> {
    if let Some(metadata) = metadata(path)? {
        if metadata.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn adopt_lowercase_dir(legacy: &Path, target: &Path) -> io::Result<()> {
    if legacy != target && metadata(legacy)?.is_some_and(|metadata| metadata.is_dir()) {
        if metadata(target)?.is_none() {
            fs::rename(legacy, target)?;
            println!("Renamed {} to {}", legacy.display(), target.display());
        } else if fs::remove_dir(legacy).is_err() {
            eprintln!(
                "Kept {} beside {}. Merge its contents manually.",
                legacy.display(),
                target.display()
            );
        }
    }
    fs::create_dir_all(target)
}

fn first_wallpaper(directory: &Path) -> io::Result<Option<PathBuf>> {
    let mut first: Option<PathBuf> = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        let candidate = if kind.is_dir() {
            first_wallpaper(&path)?
        } else if kind.is_file()
            && path.extension().is_some_and(|extension| {
                [b"png".as_slice(), b"jpg", b"jpeg", b"webp"]
                    .iter()
                    .any(|accepted| extension.as_bytes().eq_ignore_ascii_case(accepted))
            })
        {
            Some(path)
        } else {
            None
        };
        if let Some(candidate) = candidate
            && first.as_ref().is_none_or(|current| {
                candidate.as_os_str().as_bytes() < current.as_os_str().as_bytes()
            })
        {
            first = Some(candidate);
        }
    }
    Ok(first)
}

fn append_browser_defaults(destination: &mut String, browser: &str) {
    for mime in BRAVE_DEFAULTS {
        destination.push_str(mime);
        destination.push('=');
        destination.push_str(browser);
        destination.push_str(";\n");
    }
}

fn variant_contains(list: &str, wanted: &str) -> bool {
    let mut opening = None;
    let mut escaped = false;
    for (index, c) in list.char_indices() {
        if let Some((start, quote)) = opening {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                if &list[start..index] == wanted {
                    return true;
                }
                opening = None;
            }
        } else if matches!(c, '\'' | '"') {
            opening = Some((index + 1, c));
        }
    }
    false
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
