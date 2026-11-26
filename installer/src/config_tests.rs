use super::*;
use crate::{CommandOutput, Runner, error};
use std::{
    cell::RefCell,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    rc::Rc,
};
use tempfile::TempDir;

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<Vec<String>>>,
    decline: bool,
    build_fails: bool,
    binary_fails: &'static str,
    invalid_niri: bool,
    clone_fails: bool,
    clone_incomplete: bool,
    pairing_fails: bool,
    manager_running: bool,
    configured_dir: Option<PathBuf>,
    tracker_list: String,
}

impl Fake {
    fn record(&self, program: &str, args: &[&str]) {
        self.calls.borrow_mut().push(
            std::iter::once(program)
                .chain(args.iter().copied())
                .map(str::to_owned)
                .collect(),
        );
    }

    fn called(&self, program: &str, args: &[&str]) -> bool {
        self.calls.borrow().iter().any(|call| {
            call.iter()
                .map(String::as_str)
                .eq(std::iter::once(program).chain(args.iter().copied()))
        })
    }
}

impl Runner for Rc<Fake> {
    fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        self.record(program, args);
        let success = |stdout: String| {
            Ok(CommandOutput {
                stdout,
                ..Default::default()
            })
        };
        match (program, args) {
            ("cargo", ["-vV"]) => success("host: x86_64-unknown-linux-gnu\n".into()),
            ("systemctl", ["--user", "show-environment"]) if self.manager_running => {
                success("HOME=/home/example".into())
            }
            ("xdg-user-dir", ["PICTURES"]) if self.configured_dir.is_some() => {
                success(self.configured_dir.as_ref().unwrap().display().to_string())
            }
            (
                "gsettings",
                [
                    "get",
                    "org.freedesktop.Tracker3.Miner.Files",
                    "ignored-directories",
                ],
            ) if !self.tracker_list.is_empty() => success(self.tracker_list.clone()),
            _ => Ok(CommandOutput {
                code: 1,
                ..Default::default()
            }),
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        self.record(program, args);
        match (program, args) {
            ("cargo", _) if self.build_fails => Err(error("cargo failed")),
            ("niri", ["validate", "-c", path]) if self.invalid_niri => {
                assert!(Path::new(path).with_file_name("outputs.kdl").is_file());
                Err(error("invalid generated output"))
            }
            ("cargo", [targets @ .., "--target-dir", target]) => {
                let host = targets
                    .windows(2)
                    .find(|pair| pair[0] == "--target")
                    .unwrap()[1];
                let release = Path::new(target).join(host).join("release");
                fs::create_dir_all(&release)?;
                if targets.contains(&"hype-blueman-keys") {
                    fs::write(release.join(BLUEMAN_KEYS), "built module")?;
                }
                for name in targets
                    .windows(2)
                    .filter_map(|pair| (pair[0] == "--bin").then_some(pair[1]))
                {
                    let binary = release.join(name);
                    fs::write(&binary, b"staged executable")?;
                    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
                }
                Ok(())
            }
            ("git", _) if self.clone_fails => Err(error("git clone failed")),
            (
                "git",
                [
                    "clone",
                    "--depth",
                    "1",
                    "https://github.com/LazyVim/starter.git",
                    target,
                ],
            ) => {
                let target = Path::new(target);
                fs::create_dir_all(target.join(".git"))?;
                fs::create_dir_all(target.join("lua/config"))?;
                fs::write(target.join("init.lua"), "require('config.lazy')")?;
                if !self.clone_incomplete {
                    fs::write(
                        target.join("lua/config/lazy.lua"),
                        "return {'LazyVim/LazyVim'}",
                    )?;
                }
                Ok(())
            }
            ("gsettings", ["set", "org.blueman.general", "notification-daemon", "false"])
                if self.pairing_fails =>
            {
                Err(error("no active session bus"))
            }
            _ if !self.binary_fails.is_empty()
                && Path::new(program)
                    .file_name()
                    .is_some_and(|name| name == self.binary_fails) =>
            {
                Err(error("binary validation failed"))
            }
            _ => Ok(()),
        }
    }

    fn confirm(&self, _: &str) -> Result<bool> {
        Ok(!self.decline)
    }
    fn input(&self, _: &str) -> Result<String> {
        Err(error("unexpected input"))
    }
    fn available(&self, program: &str) -> bool {
        matches!(
            program,
            "gsettings" | "xdg-user-dir" | "xdg-user-dirs-update"
        )
    }
}

fn fixture(fake: Fake) -> (TempDir, Installer, Rc<Fake>) {
    let temp = tempfile::tempdir().unwrap();
    let fake = Rc::new(fake);
    let installer = Installer {
        source: temp.path().join("checkout"),
        home: temp.path().join("home"),
        root: temp.path().join("root"),
        backup: None,
        runner: Box::new(Rc::clone(&fake)),
        build_dir: None,
        optional_groups: Some(
            [
                "bluetooth",
                "docker",
                "warp",
                "editor-neovim",
                "browser-brave",
                "theme-extras",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ),
    };
    for dir in [
        "scripts",
        "niri",
        "waybar",
        "alacritty",
        "mako",
        "fastfetch",
        "hypr",
        "wallpapers",
        "systemd/user",
        "wireplumber/wireplumber.conf.d",
        "zsh",
        "fontconfig",
        "gtk",
        "waybar/icons/16x16/panel",
    ] {
        fs::create_dir_all(installer.source.join(dir)).unwrap();
    }
    for (file, contents) in [
        ("niri/config.kdl", "new Niri configuration"),
        (
            "waybar/config.jsonc",
            "{\"modules\": [\"custom/hotspot\", \"tray\"]}",
        ),
        (
            "systemd/user/hype-auto-power-profile.service",
            "new service",
        ),
        (
            "wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf",
            "new audio configuration",
        ),
        ("zsh/.zshrc", "new zshrc"),
        ("zsh/.p10k.zsh", "new p10k"),
        ("gtk/gtk.css", "dialog {}"),
        ("fontconfig/60-hype-niri-fonts.conf", "font configuration"),
    ] {
        fs::write(installer.source.join(file), contents).unwrap();
    }
    for unit in WATCHER_UNITS {
        fs::write(installer.source.join(unit), "watcher service").unwrap();
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
        fs::write(
            installer
                .source
                .join(format!("waybar/icons/16x16/panel/{icon}.svg")),
            icon,
        )
        .unwrap();
    }
    fs::create_dir(&installer.home).unwrap();
    write(
        installer.system("/usr/share/icons/Papirus-Dark/index.theme"),
        "Papirus Dark",
    );
    (temp, installer, fake)
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn declined_backup_stops_before_overwrite() {
    let (_temp, mut installer, _) = fixture(Fake {
        decline: true,
        ..Default::default()
    });
    let config = installer.user(".zshrc");
    write(config.clone(), "keep me");
    assert!(installer.backup_configs().is_err());
    assert_eq!(fs::read_to_string(config).unwrap(), "keep me");
}

#[test]
fn backup_preserves_generated_config_mimeapps_and_dangling_links() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    write(
        installer.user(".config/gtk-3.0/settings.ini"),
        "old GTK settings",
    );
    let mime = "[Default Applications]\ntext/html=firefox.desktop;\n";
    write(installer.user(".config/mimeapps.list"), mime);
    symlink("missing-zsh-target", installer.user(".zshrc")).unwrap();
    installer.backup_configs().unwrap();
    installer.set_default_browser().unwrap();
    let backup = installer.backup.unwrap();
    assert_eq!(
        fs::read_to_string(backup.join(".config/gtk-3.0/settings.ini")).unwrap(),
        "old GTK settings"
    );
    assert_eq!(
        fs::read_to_string(backup.join(".config/mimeapps.list")).unwrap(),
        mime
    );
    assert_eq!(
        fs::read_link(backup.join(".zshrc")).unwrap(),
        Path::new("missing-zsh-target")
    );
    assert!(
        fs::read_to_string(installer.home.join(".config/mimeapps.list"))
            .unwrap()
            .contains("text/html=brave-browser.desktop;")
    );
}

#[test]
fn new_backup_keeps_the_oldest_and_two_newest_generated_backups() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    for name in [
        ".config-backup-100.aaaaaa",
        ".config-backup-200.bbbbbb",
        ".config-backup-300.cccccc",
        ".config-backup-400.dddddd",
        ".config-backup-50.mine",
        ".config-backup-notes",
    ] {
        fs::create_dir(installer.home.join(name)).unwrap();
    }
    write(installer.home.join(".config-backup-10.eeeeee"), "file");
    write(installer.user(".zshrc"), "old zshrc");
    installer.backup_configs().unwrap();
    let mut left: Vec<_> = fs::read_dir(&installer.home)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".config-backup-")
        })
        .collect();
    left.sort();
    let backup = installer.backup.unwrap();
    assert!(left.contains(&backup));
    for kept in [
        ".config-backup-100.aaaaaa",
        ".config-backup-400.dddddd",
        ".config-backup-50.mine",
        ".config-backup-notes",
        ".config-backup-10.eeeeee",
    ] {
        assert!(left.contains(&installer.home.join(kept)), "{kept}");
    }
    assert_eq!(left.len(), 6);
}

#[test]
fn failed_directory_publication_leaves_previous_config() {
    let (temp, installer, _) = fixture(Fake::default());
    let old = installer.user(".config/niri/config.kdl");
    write(old.clone(), "previous config");
    assert!(replace_directory(&temp.path().join("missing-stage"), old.parent().unwrap()).is_err());
    assert_eq!(fs::read_to_string(old).unwrap(), "previous config");
}

#[test]
fn build_and_binary_validation_failures_leave_configs_untouched() {
    for binary in ["cargo", "hype-notch", "hype-control", "hype-status"] {
        let (_temp, mut installer, _) = fixture(Fake {
            build_fails: binary == "cargo",
            binary_fails: binary,
            ..Default::default()
        });
        let old = installer.user(".config/niri/config.kdl");
        write(old.clone(), "previous config");
        assert!(installer.copy_configs().is_err(), "{binary}");
        assert_eq!(fs::read_to_string(old).unwrap(), "previous config");
        assert!(!installer.user(".config/scripts").exists(), "{binary}");
    }
}

#[test]
fn only_the_stealth_build_requests_its_binary_and_uses_the_desktop_cache() {
    let (_temp, mut installer, fake) = fixture(Fake::default());
    let release = installer.build_programs().unwrap();
    assert!(!release.join("stealth").exists());
    let stealth = installer.build_stealth().unwrap();
    assert!(stealth.path().join("stealth").exists());
    assert!(!release.join("stealth").exists());
    let calls = fake.calls.borrow();
    let builds: Vec<_> = calls
        .iter()
        .filter(|call| call[..2] == ["cargo", "build"])
        .collect();
    assert_eq!(builds.len(), 2);
    assert_eq!(builds[0].last(), builds[1].last());
}

#[test]
fn failed_niri_stage_keeps_existing_scripts_and_configuration() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    fs::remove_dir_all(installer.source.join("niri")).unwrap();
    let old = installer.user(".config/niri/config.kdl");
    write(old.clone(), "previous config");
    let legacy = installer.user(".config/scripts/start-waybar.sh");
    write(legacy.clone(), "original launcher");
    assert!(installer.copy_configs().is_err());
    assert_eq!(fs::read_to_string(old).unwrap(), "previous config");
    assert_eq!(fs::read_to_string(legacy).unwrap(), "original launcher");
}

#[test]
fn invalid_staged_config_is_rejected_before_any_publication() {
    let (_temp, mut installer, _) = fixture(Fake {
        invalid_niri: true,
        ..Default::default()
    });
    let old = installer.user(".config/niri/config.kdl");
    let outputs = installer.user(".config/niri/outputs.kdl");
    let launcher = installer.user(".config/scripts/start-waybar.sh");
    write(old.clone(), "previous config");
    write(outputs.clone(), "saved monitor config");
    write(launcher.clone(), "original launcher");
    let message = installer.copy_configs().unwrap_err().to_string();
    assert!(message.contains("Staged Niri configuration is invalid"));
    assert_eq!(fs::read_to_string(old).unwrap(), "previous config");
    assert_eq!(fs::read_to_string(outputs).unwrap(), "saved monitor config");
    assert_eq!(fs::read_to_string(launcher).unwrap(), "original launcher");
    assert!(!installer.user(".config/waybar").exists());
}

#[test]
fn failed_publication_restores_all_earlier_directories() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir_in(root.path()).unwrap();
    let config = root.path().join("config");
    write(config.join("first/setting"), "old");
    write(stage.path().join("first/setting"), "new");
    write(stage.path().join("extra/setting"), "new directory");
    assert!(publish_config_dirs(stage, &config, &["first", "extra", "missing"]).is_err());
    assert_eq!(
        fs::read_to_string(config.join("first/setting")).unwrap(),
        "old"
    );
    assert!(!config.join("extra").exists());
}

#[test]
fn binaries_are_executable_and_legacy_links_retire_after_publish() {
    let (_temp, mut installer, fake) = fixture(Fake::default());
    write(
        installer.source.join("scripts/fullscreen-toggle.sh"),
        "legacy source",
    );
    installer.copy_configs().unwrap();
    let scripts = installer.user(".config/scripts");
    assert_ne!(
        scripts
            .join("hype-notch")
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert!(!scripts.join("notch.sh").is_symlink());
    assert!(!scripts.join("fullscreen-toggle.sh").exists());
    for helper in WAYBAR_COMPATIBILITY
        .iter()
        .copied()
        .chain(["prepare-sleep"])
    {
        assert_eq!(
            fs::read_link(scripts.join(format!("{helper}.sh"))).unwrap(),
            Path::new("hype-control")
        );
    }
    // Older installed units invoke this path through Bash, so a symlink to ELF is unusable.
    fs::write(
        scripts.join("hype-control"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\"\n",
    )
    .unwrap();
    let output = std::process::Command::new("bash")
        .arg(scripts.join("auto-power-profile.sh"))
        .arg("probe")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"auto-power-profile probe\n");
    installer.setup_desktop_integrations().unwrap();
    assert!(!scripts.join("auto-power-profile.sh").exists());
    installer.build_programs().unwrap();
    assert_eq!(
        fake.calls
            .borrow()
            .iter()
            .filter(|call| call[..2] == ["cargo", "build"])
            .count(),
        1
    );
}

#[test]
fn reinstall_preserves_monitor_settings_and_moves_existing_drive_links() {
    let (temp, mut installer, _) = fixture(Fake::default());
    let saved = installer.user(".config/niri/outputs.kdl");
    write(saved.clone(), "output settings");
    let drives = installer.user("Drives");
    fs::create_dir(&drives).unwrap();
    for (name, target) in [
        ("Work", temp.path().to_path_buf()),
        ("Offline", PathBuf::from("/missing/custom-drive")),
        ("Windows-SSD", PathBuf::from("/run/media/user/Windows-SSD")),
    ] {
        symlink(&target, drives.join(name)).unwrap();
    }
    installer.copy_configs().unwrap();
    assert_eq!(fs::read_to_string(saved).unwrap(), "output settings");
    assert_eq!(
        fs::read_to_string(installer.user(".config/niri/config.kdl")).unwrap(),
        "new Niri configuration"
    );
    assert!(!drives.exists());
    assert_eq!(
        fs::read_link(installer.user("drives/Work")).unwrap(),
        temp.path()
    );
    assert_eq!(
        fs::read_link(installer.user("drives/Offline")).unwrap(),
        Path::new("/missing/custom-drive")
    );
    assert_eq!(
        fs::read_link(installer.user("drives/Windows-SSD")).unwrap(),
        Path::new("/run/media/user/Windows-SSD")
    );
}

#[test]
fn republish_keeps_live_opacity_or_the_repository_default() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    write(installer.source.join("niri/opacity.kdl"), "default opacity");
    let live = installer.user(".config/niri/opacity.kdl");
    installer.copy_configs().unwrap();
    assert_eq!(fs::read_to_string(&live).unwrap(), "default opacity");
    fs::write(&live, "user opacity").unwrap();
    installer.copy_configs().unwrap();
    assert_eq!(fs::read_to_string(&live).unwrap(), "user opacity");
}

#[test]
fn wallpaper_seed_handles_large_directories_and_keeps_existing_files_and_pointer() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    let wallpapers = installer.user("pictures/wallpapers");
    let first = wallpapers.join("00000 wallpaper.jpg");
    write(first.clone(), "user wallpaper");
    for index in 1..5000 {
        fs::File::create(wallpapers.join(format!("{index:05} wallpaper.jpg"))).unwrap();
    }
    write(
        installer.source.join("wallpapers/00000 wallpaper.jpg"),
        "repository wallpaper",
    );
    installer.copy_configs().unwrap();
    let pointer = installer.user(".local/state/niri/current_wallpaper");
    assert_eq!(fs::read_to_string(first.clone()).unwrap(), "user wallpaper");
    assert_eq!(fs::read_link(&pointer).unwrap(), first);
    assert!(installer.user(".config/niri/outputs.kdl").exists());
    fs::remove_file(&pointer).unwrap();
    let chosen = wallpapers.join("00001 wallpaper.jpg");
    symlink(&chosen, &pointer).unwrap();
    installer.copy_configs().unwrap();
    assert_eq!(fs::read_link(pointer).unwrap(), chosen);
}

#[test]
fn user_data_symlinks_stay_linked_without_copying_unrelated_collections() {
    let (temp, mut installer, _) = fixture(Fake::default());
    for (name, entry) in [
        ("pictures", "photo-collection"),
        (".cache", "unrelated-cache"),
        (".local", "other-application-data"),
    ] {
        let external = temp.path().join(format!("external-{entry}"));
        fs::create_dir(&external).unwrap();
        fs::write(external.join(entry), "keep original data").unwrap();
        symlink(&external, installer.user(name)).unwrap();
    }
    installer.copy_configs().unwrap();
    for name in ["pictures", ".cache", ".local"] {
        assert!(
            installer.user(name).is_symlink(),
            "{name} was replaced with a recursive copy"
        );
    }
    assert!(installer.user("pictures/wallpapers").is_dir());
    assert!(!installer.user(".cache/cliphist").exists());
}

#[test]
fn configs_phase_backs_up_existing_user_files_first() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    write(installer.user(".config/niri/config.kdl"), "user Niri");
    installer.run_phase("configs").unwrap();
    assert_eq!(
        fs::read_to_string(installer.backup.unwrap().join(".config/niri/config.kdl")).unwrap(),
        "user Niri"
    );
}

#[test]
fn top_level_config_symlink_target_remains_untouched() {
    let (temp, mut installer, _) = fixture(Fake::default());
    let external = temp.path().join("external-dotfiles");
    write(external.join("niri/config.kdl"), "external Niri");
    write(external.join("sentinel"), "unrelated settings");
    symlink(&external, installer.user(".config")).unwrap();
    installer.backup_configs().unwrap();
    installer.copy_configs().unwrap();
    assert_eq!(
        fs::read_to_string(external.join("niri/config.kdl")).unwrap(),
        "external Niri"
    );
    assert_eq!(
        fs::read_to_string(external.join("sentinel")).unwrap(),
        "unrelated settings"
    );
    assert!(!installer.user(".config").is_symlink());
    assert_eq!(
        fs::read_to_string(installer.user(".config/sentinel")).unwrap(),
        "unrelated settings"
    );
    assert_eq!(
        fs::read_to_string(installer.backup.unwrap().join(".config/niri/config.kdl")).unwrap(),
        "external Niri"
    );
}

#[test]
fn generated_gtk_theme_detaches_directory_and_safely_replaces_settings_link() {
    let (temp, mut installer, _) = fixture(Fake::default());
    let original = temp.path().join("external-theme");
    write(original.join("settings.ini"), "original GTK3 settings");
    let gtk4 = temp.path().join("external-gtk4-settings");
    write(gtk4.clone(), "original GTK4 settings");
    fs::create_dir_all(installer.user(".config/gtk-4.0")).unwrap();
    symlink(&original, installer.user(".config/gtk-3.0")).unwrap();
    symlink(&gtk4, installer.user(".config/gtk-4.0/settings.ini")).unwrap();
    installer.setup_gtk().unwrap();
    assert!(!installer.user(".config/gtk-3.0").is_symlink());
    assert!(
        fs::read_to_string(installer.user(".config/gtk-3.0/settings.ini"))
            .unwrap()
            .contains("Adwaita-dark")
    );
    assert_eq!(
        fs::read_to_string(original.join("settings.ini")).unwrap(),
        "original GTK3 settings"
    );
    assert_eq!(fs::read_to_string(gtk4).unwrap(), "original GTK4 settings");
    assert_eq!(
        fs::read_link(installer.user(".local/share/icons/Papirus-Dark/16x16@2x")).unwrap(),
        Path::new("16x16")
    );
}

#[test]
fn user_dirs_adopt_legacy_and_keep_nonempty_conflicts_and_external_paths() {
    let external = tempfile::tempdir().unwrap();
    let (_temp, mut installer, fake) = fixture(Fake {
        configured_dir: Some(external.path().to_path_buf()),
        ..Default::default()
    });
    write(installer.user("Pictures/Wallpapers/kept.jpg"), "wallpaper");
    write(installer.user("Videos/clip.mp4"), "clip");
    fs::create_dir(installer.user("videos")).unwrap();
    fs::create_dir(installer.user("Music")).unwrap();
    installer.setup_user_dirs().unwrap();
    assert!(external.path().is_dir());
    assert!(!installer.user("Pictures").exists());
    assert!(!installer.user("Music").exists());
    assert_eq!(
        fs::read_to_string(installer.user("pictures/Wallpapers/kept.jpg")).unwrap(),
        "wallpaper"
    );
    assert!(installer.user("Videos/clip.mp4").exists());
    for dir in [
        "desktop",
        "documents",
        "downloads",
        "music",
        "projects",
        "public",
        "templates",
    ] {
        assert!(installer.user(dir).is_dir(), "{dir}");
    }
    for (name, dir) in [("PICTURES", "pictures"), ("DOWNLOAD", "downloads")] {
        assert!(fake.called(
            "xdg-user-dirs-update",
            &["--set", name, installer.user(dir).to_str().unwrap()]
        ));
    }
}

#[test]
fn lazyvim_clones_once_backs_up_old_settings_and_removes_git_metadata() {
    let (_temp, mut installer, fake) = fixture(Fake::default());
    let config = installer.user(".config/nvim");
    write(config.join("init.lua"), "old settings");
    installer.setup_lazyvim().unwrap();
    let lazy = config.join("lua/config/lazy.lua");
    assert!(lazy.is_file());
    assert!(!config.join(".git").exists());
    assert_eq!(
        fs::read_to_string(
            installer
                .backup
                .clone()
                .unwrap()
                .join(".config/nvim/init.lua")
        )
        .unwrap(),
        "old settings"
    );
    fs::write(&lazy, "-- customized\nreturn {'LazyVim/LazyVim'}").unwrap();
    installer.setup_lazyvim().unwrap();
    assert!(
        fs::read_to_string(lazy)
            .unwrap()
            .starts_with("-- customized")
    );
    assert_eq!(
        fake.calls
            .borrow()
            .iter()
            .filter(|call| call[0] == "git")
            .count(),
        1
    );
}

#[test]
fn failed_or_incomplete_lazyvim_clone_keeps_previous_config_and_cleans_stage() {
    for clone_fails in [false, true] {
        let (_temp, mut installer, _) = fixture(Fake {
            clone_fails,
            clone_incomplete: !clone_fails,
            ..Default::default()
        });
        let old = installer.user(".config/nvim/init.lua");
        write(old.clone(), "old settings");
        assert!(installer.setup_lazyvim().is_err());
        assert_eq!(fs::read_to_string(old).unwrap(), "old settings");
        assert_eq!(fs::read_dir(installer.user(".config")).unwrap().count(), 1);
    }
}

#[test]
fn default_browser_updates_mime_defaults_idempotently_and_keeps_other_settings() {
    let (_temp, installer, _) = fixture(Fake::default());
    let mime = installer.user(".config/mimeapps.list");
    installer.set_default_browser().unwrap();
    assert!(
        fs::read_to_string(&mime)
            .unwrap()
            .starts_with("[Default Applications]\n")
    );
    write(
        mime.clone(),
        "[Default Applications]\nx-scheme-handler/claude-cli=claude-code-url-handler.desktop\ntext/html=firefox.desktop\n\n[Added Associations]\nimage/png=feh.desktop;\n",
    );
    installer.set_default_browser().unwrap();
    let once = fs::read_to_string(&mime).unwrap();
    installer.set_default_browser().unwrap();
    assert_eq!(fs::read_to_string(mime).unwrap(), once);
    let (defaults, added) = once.split_once("[Added Associations]").unwrap();
    assert!(added.contains("image/png=feh.desktop;"));
    assert!(defaults.contains("x-scheme-handler/claude-cli=claude-code-url-handler.desktop"));
    assert!(!defaults.contains("firefox.desktop"));
    for mime in [
        "text/html",
        "x-scheme-handler/http",
        "x-scheme-handler/https",
    ] {
        assert_eq!(
            defaults
                .lines()
                .filter(|line| *line == format!("{mime}=brave-browser.desktop;"))
                .count(),
            1
        );
    }
}

#[test]
fn default_browser_keeps_original_mimeapps_after_read_error() {
    let (_temp, installer, _) = fixture(Fake::default());
    let mime = installer.user(".config/mimeapps.list");
    fs::create_dir_all(mime.parent().unwrap()).unwrap();
    let original = b"[Default Applications]\ntext/html=firefox.desktop;\n\xff";
    fs::write(&mime, original).unwrap();
    assert!(installer.set_default_browser().is_err());
    assert_eq!(fs::read(mime).unwrap(), original);
}

#[test]
fn bluetooth_pairing_failure_allows_service_and_audio_config_installation() {
    let (temp, mut installer, fake) = fixture(Fake {
        pairing_fails: true,
        ..Default::default()
    });
    let external = temp.path().join("external-systemd");
    write(
        external.join("user/hype-auto-power-profile.service"),
        "old service",
    );
    fs::create_dir(installer.user(".config")).unwrap();
    symlink(&external, installer.user(".config/systemd")).unwrap();
    installer.setup_desktop_integrations().unwrap();
    assert!(fake.called(
        "gsettings",
        &["set", "org.blueman.general", "notification-daemon", "false"]
    ));
    assert_eq!(
        fs::read_to_string(external.join("user/hype-auto-power-profile.service")).unwrap(),
        "old service"
    );
    let wants = installer.user(
        ".config/systemd/user/graphical-session.target.wants/hype-auto-power-profile.service",
    );
    assert_eq!(
        fs::read_link(&wants).unwrap(),
        Path::new("../hype-auto-power-profile.service")
    );
    assert_eq!(fs::read_to_string(wants).unwrap(), "new service");
    assert_eq!(
        fs::read_to_string(
            installer.user(".config/wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf")
        )
        .unwrap(),
        "new audio configuration"
    );
    assert!(!fake.called("systemctl", &["--user", "daemon-reload"]));
}

#[test]
fn blueman_cleanup_removes_old_wrappers_and_keeps_user_overrides() {
    let (_temp, mut installer, fake) = fixture(Fake {
        manager_running: true,
        ..Default::default()
    });
    let entry = installer.user(".local/share/applications/blueman-manager.desktop");
    let dropin = installer.user(".config/systemd/user/blueman-manager.service.d");
    write(
        entry.clone(),
        "[Desktop Entry]\nExec=/usr/bin/python3 \"/some/path/.config/scripts/blueman-manager.py\"\n",
    );
    write(
        dropin.join("hype-niri.conf"),
        "[Service]\nExecStart=\nExecStart=/usr/bin/python3 %h/.config/scripts/blueman-manager.py\n",
    );
    installer.optional_groups = Some(Vec::new());
    installer.setup_desktop_integrations().unwrap();
    assert!(!entry.exists());
    assert!(!dropin.join("hype-niri.conf").exists());
    assert!(!fake.called(
        "gsettings",
        &["set", "org.blueman.general", "notification-daemon", "false"]
    ));
    assert!(fake.called("systemctl", &["--user", "daemon-reload"]));
    let custom = "[Desktop Entry]\nExec=blueman-manager --custom\n";
    write(entry.clone(), custom);
    write(dropin.join("hype-niri.conf"), "installer override");
    write(dropin.join("mine.conf"), "custom override");
    installer.configure_blueman_wrapper().unwrap();
    assert_eq!(fs::read_to_string(entry).unwrap(), custom);
    assert_eq!(fs::read_dir(&dropin).unwrap().count(), 2);
    assert_eq!(
        fs::read_to_string(dropin.join("mine.conf")).unwrap(),
        "custom override"
    );
}

#[test]
fn existing_blueman_gains_native_escape_when_extra_is_skipped() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    installer.optional_groups = Some(Vec::new());
    write(
        installer.system("/usr/share/applications/blueman-manager.desktop"),
        "[Desktop Entry]\nName=Bluetooth Manager\nExec=blueman-manager\n",
    );
    write(
        installer.user(".config/scripts").join(BLUEMAN_KEYS),
        "module",
    );
    let user_entry = installer.user(".local/share/applications/blueman-manager.desktop");
    write(
        user_entry.clone(),
        "[Desktop Entry]\nName=My Bluetooth\nExec=/usr/bin/python3 \"/some/path/.config/scripts/blueman-manager.py\"\n",
    );
    installer.setup_desktop_integrations().unwrap();
    let entry =
        fs::read_to_string(installer.user(".local/share/applications/blueman-manager.desktop"))
            .unwrap();
    assert!(entry.contains("Name=My Bluetooth"));
    assert!(entry.contains("/.config/scripts/hype-control\" blueman-manager"));
    assert!(!entry.contains("python3"));
    let dropin = installer.user(".config/systemd/user/blueman-manager.service.d/hype-niri.conf");
    assert!(
        fs::read_to_string(dropin)
            .unwrap()
            .contains("%h/.config/scripts/hype-control blueman-manager")
    );
}

#[test]
fn updater_rebuilds_and_replaces_its_registered_binary() {
    let (_temp, installer, fake) = fixture(Fake::default());
    let first = installer.home.join("previous-installer");
    write(first.clone(), "previous executable");
    let launcher = installer.publish_cli(&first).unwrap();
    installer.refresh_cli().unwrap();
    assert_eq!(fs::read_to_string(launcher).unwrap(), "staged executable");
    assert!(fake.calls.borrow().iter().any(|call| {
        call[..2] == ["cargo", "build"]
            && call
                .windows(2)
                .any(|args| args == ["--package", "hype-install"])
    }));
}

#[test]
fn updater_keeps_the_previous_binary_if_the_new_build_fails() {
    let (_temp, installer, _) = fixture(Fake {
        build_fails: true,
        ..Default::default()
    });
    let first = installer.home.join("previous-installer");
    write(first.clone(), "previous executable");
    let launcher = installer.publish_cli(&first).unwrap();
    assert!(installer.refresh_cli().is_err());
    assert_eq!(fs::read_to_string(launcher).unwrap(), "previous executable");
}

#[test]
fn stale_audio_service_removes_legacy_links_without_touching_sources() {
    for manager_running in [false, true] {
        let (temp, installer, fake) = fixture(Fake {
            manager_running,
            ..Default::default()
        });
        let external = temp.path().join("external-legacy.service");
        write(
            external.clone(),
            "[Service]\nExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh\n",
        );
        let unit = installer.user(".config/systemd/user/audio-jack-switch.service");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        symlink(&external, &unit).unwrap();
        let wants =
            installer.user(".config/systemd/user/default.target.wants/audio-jack-switch.service");
        fs::create_dir_all(wants.parent().unwrap()).unwrap();
        symlink("../audio-jack-switch.service", &wants).unwrap();
        installer.remove_stale_audio_service().unwrap();
        assert!(!unit.is_symlink());
        assert!(external.is_file());
        if manager_running {
            assert!(fake.called(
                "systemctl",
                &["--user", "disable", "--now", "audio-jack-switch.service"]
            ));
        } else {
            assert!(!wants.is_symlink());
        }
    }
}

#[test]
fn audio_cleanup_leaves_working_units_and_other_commands() {
    for (command, script_mode) in [
        (
            "ExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh",
            Some(0o755),
        ),
        (
            "ExecStart=%h/.config/waybar/scripts/audio-jack-switch.sh",
            Some(0o644),
        ),
        (
            "ExecStart=%h/xconfig/waybar/scripts/audio-jack-switchXsh",
            None,
        ),
    ] {
        let (_temp, installer, fake) = fixture(Fake {
            manager_running: true,
            ..Default::default()
        });
        let unit = installer.user(".config/systemd/user/audio-jack-switch.service");
        write(unit.clone(), command);
        if let Some(mode) = script_mode {
            let script = installer.user(".config/waybar/scripts/audio-jack-switch.sh");
            write(script.clone(), "existing script");
            fs::set_permissions(script, fs::Permissions::from_mode(mode)).unwrap();
        }
        installer.remove_stale_audio_service().unwrap();
        assert_eq!(fs::read_to_string(unit).unwrap(), command);
        assert!(fake.calls.borrow().is_empty());
    }
}

#[test]
fn tracker_indexing_preserves_user_exclusions_and_appends_only_missing_entries() {
    let wanted = "'node_modules', '.venv', 'venv', 'miniconda3', '__pycache__', 'target', '.mypy_cache', '.pytest_cache', '.ruff_cache'";
    let all = format!("['po', {wanted}]");
    let from_empty = format!("[{wanted}]");
    let from_partial = format!("['po', 'venv', {}]", wanted.replacen("'venv', ", "", 1));
    for (current, expected) in [
        ("@as []", Some(from_empty.as_str())),
        ("['po', 'venv']", Some(from_partial.as_str())),
        (all.as_str(), None),
        ("", None),
    ] {
        let (_temp, installer, fake) = fixture(Fake {
            tracker_list: current.into(),
            ..Default::default()
        });
        installer.configure_file_indexing().unwrap();
        let calls = fake.calls.borrow();
        let writes: Vec<_> = calls
            .iter()
            .filter(|call| call.get(1).is_some_and(|arg| arg == "set"))
            .collect();
        if let Some(value) = expected {
            assert_eq!(writes.len(), 1);
            assert_eq!(
                writes[0],
                &[
                    "gsettings",
                    "set",
                    "org.freedesktop.Tracker3.Miner.Files",
                    "ignored-directories",
                    value
                ]
            );
        } else {
            assert!(writes.is_empty());
        }
    }
}

#[test]
fn core_selection_skips_optional_app_setup_and_persists_empty_tray_policy() {
    let (_temp, mut installer, fake) = fixture(Fake::default());
    installer.optional_groups = Some(Vec::new());
    installer.setup_lazyvim().unwrap();
    installer.setup_desktop_integrations().unwrap();
    assert!(!installer.user(".config/nvim").exists());
    assert!(!installer.user(".config/mimeapps.list").exists());
    assert!(!fake.called(
        "gsettings",
        &["set", "org.blueman.general", "notification-daemon", "false"]
    ));
    installer.copy_configs().unwrap();
    assert_eq!(
        fs::read(installer.user(".config/hype-niri/optional-groups")).unwrap(),
        b""
    );
    let waybar = fs::read_to_string(installer.user(".config/waybar/config.jsonc")).unwrap();
    assert!(!waybar.contains("warp") && !waybar.contains("stealth"));
    assert_eq!(
        installer
            .user(".config/hype-niri/optional-groups")
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    installer.setup_gtk().unwrap();
    assert!(
        fs::read_to_string(installer.user(".config/gtk-4.0/settings.ini"))
            .unwrap()
            .contains("gtk-cursor-theme-name=Adwaita\n")
    );
    assert_eq!(
        fs::read_to_string(installer.user(".config/gtk-3.0/gtk.css")).unwrap(),
        "dialog {}"
    );
}

#[test]
fn selected_firefox_sets_firefox_defaults_and_persists_selected_groups() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    installer.optional_groups = Some(vec!["browser-firefox".into(), "bluetooth".into()]);
    installer.set_default_browser().unwrap();
    let defaults = fs::read_to_string(installer.user(".config/mimeapps.list")).unwrap();
    assert!(defaults.contains("text/html=firefox.desktop;"));
    assert!(!defaults.contains("brave"));
    installer.copy_configs().unwrap();
    assert_eq!(
        fs::read_to_string(installer.user(".config/hype-niri/optional-groups")).unwrap(),
        "browser-firefox\nbluetooth\n"
    );
}

#[test]
fn selected_vpn_indicators_are_added_to_the_staged_waybar_group() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    installer.optional_groups = Some(vec!["warp".into(), "stealth".into()]);
    installer.copy_configs().unwrap();
    assert!(
        fs::read_to_string(installer.user(".config/waybar/config.jsonc"))
            .unwrap()
            .contains(
                "\"modules\": [\"custom/hotspot\", \"tray\", \"custom/warp\", \"image#stealth\"]"
            )
    );
}

#[test]
fn package_phase_selection_can_be_saved_for_later_manual_phases() {
    let (_temp, mut installer, _fake) = fixture(Fake::default());
    installer.optional_groups = Some(vec!["bluetooth".into(), "editor-neovim".into()]);
    installer.save_optional_groups().unwrap();
    let path = installer.user(".config/hype-niri/optional-groups");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "bluetooth\neditor-neovim\n"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    installer.optional_groups = Some(Vec::new());
    installer.save_optional_groups().unwrap();
    assert!(fs::read_to_string(path).unwrap().is_empty());
}

#[test]
fn optional_cursor_theme_is_applied_only_when_selected() {
    let (_temp, mut installer, _) = fixture(Fake::default());
    fs::write(
        installer.source.join("niri/config.kdl"),
        "cursor {\n    xcursor-theme \"Adwaita\"\n}\n",
    )
    .unwrap();
    installer.optional_groups = Some(vec!["theme-extras".into()]);
    installer.copy_configs().unwrap();
    assert!(
        fs::read_to_string(installer.user(".config/niri/config.kdl"))
            .unwrap()
            .contains("xcursor-theme \"Bibata-Modern-Ice\"")
    );
    installer.optional_groups = Some(Vec::new());
    installer.copy_configs().unwrap();
    assert!(
        fs::read_to_string(installer.user(".config/niri/config.kdl"))
            .unwrap()
            .contains("xcursor-theme \"Adwaita\"")
    );
}
