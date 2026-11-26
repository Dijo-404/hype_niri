use hype_install::{CommandOutput, Installer, Result, Runner, error};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fs::{self, File},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

const OOM_SOURCE: &str = "systemd/user@.service.d/60-hype-niri-oomd.conf";
const OOM_DEST: &str = "/etc/systemd/system/user@.service.d/60-hype-niri-oomd.conf";
const OOM_POLICY: &str = "MemoryAccounting=yes\nManagedOOMMemoryPressure=kill\nManagedOOMSwap=kill\nManagedOOMMemoryPressureLimit=1717986918\nManagedOOMMemoryPressureDurationUSec=10s\n";

#[derive(Default)]
struct Calls {
    log: Vec<String>,
    overrides: HashMap<String, VecDeque<(i32, String)>>,
    answer: String,
    unavailable: Vec<String>,
}

struct Fake {
    calls: Rc<RefCell<Calls>>,
    fixture: PathBuf,
}

impl Runner for Fake {
    fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let call = format!("{program} {}", args.join(" "));
        let mut calls = self.calls.borrow_mut();
        calls.log.push(call.clone());
        let key = if program == "tor"
            && args
                .last()
                .is_some_and(|path| path.ends_with("torrc.preview"))
        {
            "tor preview"
        } else if args.contains(&"--install-stealth") {
            "elevated stealth install"
        } else if args == ["version"] {
            "stealth version"
        } else {
            &call
        };
        let (code, stdout) =
            if let Some(response) = calls.overrides.get_mut(key).and_then(VecDeque::pop_front) {
                response
            } else if program == "cp" {
                assert!(
                    args[args.len() - 2..]
                        .iter()
                        .all(|p| Path::new(p).starts_with(&self.fixture))
                );
                (
                    i32::from(!Command::new("cp").args(args).status()?.success()),
                    String::new(),
                )
            } else if args == ["version"] {
                (0, "stealth-transparent-v2\n".into())
            } else if program == "cargo" && args == ["-vV"] {
                (0, "host: x86_64-unknown-linux-gnu\n".into())
            } else if program == "systemctl" && args.contains(&"list-unit-files") {
                (0, format!("{} enabled\n", args[args.len() - 2]))
            } else if program == "systemctl" && args == ["--version"] {
                (0, "systemd 257 (257)\n".into())
            } else if program == "systemctl" && args.first() == Some(&"show") {
                (0, OOM_POLICY.into())
            } else if call.starts_with("systemctl is-active --quiet user@")
                || matches!(
                    call.as_str(),
                    "systemctl --user show-environment"
                        | "systemctl is-active --quiet stealth.service"
                        | "systemctl is-active --quiet privacy-shield.service"
                        | "systemctl is-active --quiet tor.service"
                        | "nft list table inet stealth"
                        | "nft list table inet privacy_shield"
                        | "systemctl is-enabled --quiet docker.service"
                        | "getent group docker"
                )
            {
                (3, String::new())
            } else if call == "visudo -c" {
                (
                    0,
                    format!(
                        "{}: parsed OK\n",
                        self.fixture.join("root/etc/sudoers.d/stealth").display()
                    ),
                )
            } else {
                (0, String::new())
            };
        Ok(CommandOutput {
            code,
            stdout,
            stderr: String::new(),
        })
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        if self.output(program, args)?.success() {
            Ok(())
        } else {
            Err(error(format!("{program} {}", args.join(" "))))
        }
    }

    fn confirm(&self, _: &str) -> Result<bool> {
        Ok(true)
    }

    fn confirm_explicit(&self, _: &str) -> Result<bool> {
        Ok(true)
    }

    fn input(&self, _: &str) -> Result<String> {
        Ok(self.calls.borrow().answer.clone())
    }

    fn available(&self, program: &str) -> bool {
        !self
            .calls
            .borrow()
            .unavailable
            .iter()
            .any(|name| name == program)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    calls: Rc<RefCell<Calls>>,
    installer: Installer,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let calls = Rc::new(RefCell::new(Calls::default()));
        let installer = Installer {
            source: dir.path().join("source"),
            home: dir.path().join("home with spaces"),
            root: dir.path().join("root"),
            backup: None,
            runner: Box::new(Fake {
                calls: calls.clone(),
                fixture: dir.path().into(),
            }),
            build_dir: None,
            optional_groups: Some(
                [
                    "bluetooth",
                    "docker",
                    "warp",
                    "stealth",
                    "editor-neovim",
                    "browser-brave",
                    "theme-extras",
                    "login-manager-ly",
                    "firewall",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
        };
        Self {
            _dir: dir,
            calls,
            installer,
        }
    }

    fn override_call(&self, command: &str, responses: &[(i32, &str)]) {
        self.calls.borrow_mut().overrides.insert(
            command.into(),
            responses
                .iter()
                .map(|(code, output)| (*code, (*output).into()))
                .collect(),
        );
    }

    fn log(&self) -> Vec<String> {
        self.calls.borrow().log.clone()
    }

    fn prepare_oom(&self) {
        put(
            &self.installer.system("/sys/fs/cgroup/cgroup.controllers"),
            "cpu memory io\n",
        );
        put(
            &self.installer.system("/proc/pressure/memory"),
            "some avg10=0.00\n",
        );
        put(
            &self.installer.source.join(OOM_SOURCE),
            "[Service]\nManagedOOMSwap=kill\n",
        );
    }

    fn prepare_system(&self) {
        self.prepare_oom();
        put(
            &self.installer.source.join("polkit/60-hype.rules"),
            "// polkit fixture\n",
        );
        self.override_call(
            "systemctl list-unit-files --type=service --no-legend",
            &[(0, "ly@.service disabled\nsddm.service enabled\n")],
        );
    }

    fn prepare_stealth(&self) -> PathBuf {
        let root = &self.installer.root;
        fs::create_dir_all(root.join("run")).unwrap();
        for (path, content) in [
            ("/usr/local/bin/privacy-shield.sh", "original script"),
            (
                "/etc/systemd/system/privacy-shield.service",
                "original unit",
            ),
            ("/etc/privacy-shield.nft", "original rules"),
            (
                "/etc/tor/torrc.d/privacy-shield.conf",
                "original Tor listener config",
            ),
            (
                "/etc/tor/torrc",
                "User tor\n%include /etc/tor/torrc.d/privacy-shield.conf\n",
            ),
            ("/var/log/privacy-shield.log", "old log"),
        ] {
            put(&self.installer.system(path), content);
        }
        let payload = self.installer.source.join("stealth");
        for name in [
            "stealth.service",
            "stealth.nft",
            "stealth.sudoers",
            "torrc.conf",
        ] {
            let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../stealth")
                .join(name);
            put(&payload.join(name), fs::read(source).unwrap());
        }
        // Use an ELF header and provide the binary's version through the fake process.
        put(&payload.join("stealth"), b"\x7fELFtest");
        fs::set_permissions(payload.join("stealth"), fs::Permissions::from_mode(0o755)).unwrap();
        payload
    }

    fn assert_legacy_restored(&self) {
        for (path, content) in [
            ("/usr/local/bin/privacy-shield.sh", "original script"),
            (
                "/etc/systemd/system/privacy-shield.service",
                "original unit",
            ),
            ("/etc/privacy-shield.nft", "original rules"),
            (
                "/etc/tor/torrc.d/privacy-shield.conf",
                "original Tor listener config",
            ),
            (
                "/etc/tor/torrc",
                "User tor\n%include /etc/tor/torrc.d/privacy-shield.conf\n",
            ),
            ("/var/log/privacy-shield.log", "old log"),
        ] {
            assert_eq!(
                fs::read_to_string(self.installer.system(path)).unwrap(),
                content
            );
        }
        for path in [
            "/usr/local/bin/stealth",
            "/etc/systemd/system/stealth.service",
            "/etc/stealth.nft",
            "/etc/tor/torrc.d/stealth.conf",
            "/etc/sudoers.d/stealth",
            "/var/log/stealth.log",
        ] {
            assert!(!self.installer.system(path).exists(), "{path} still exists");
        }
    }
}

fn put(path: &Path, data: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

#[test]
fn rejects_unusable_pacman_config_before_services() {
    let mut f = Fixture::new();
    put(
        &f.installer.system("/etc/pacman.conf"),
        "# No options section\n",
    );
    assert!(
        f.installer
            .setup_system()
            .unwrap_err()
            .to_string()
            .contains("[options]")
    );
    assert!(f.log().is_empty());
}

#[test]
fn ly_enable_failure_keeps_previous_manager() {
    let mut f = Fixture::new();
    f.prepare_system();
    f.override_call("sudo systemctl enable --force ly@tty2.service", &[(1, "")]);
    assert!(f.installer.setup_system().is_err());
    let log = f.log();
    assert!(log.contains(&"sudo systemctl enable --force ly@tty2.service".into()));
    assert!(
        !log.iter()
            .any(|call| call.starts_with("sudo systemctl disable"))
    );
}

#[test]
fn enables_ly_before_retiring_the_previous_manager_and_getty() {
    let mut f = Fixture::new();
    f.prepare_system();
    put(
        &f.installer.system("/etc/pacman.conf"),
        "[options]\n#Color\n#ParallelDownloads = 5\n",
    );
    f.installer.setup_system().unwrap();
    let log = f.log();
    let position = |call: &str| log.iter().position(|entry| entry == call).unwrap();
    assert!(
        position("sudo systemctl enable --force ly@tty2.service")
            < position("sudo systemctl disable sddm.service")
    );
    assert!(
        position("sudo systemctl disable sddm.service")
            < position("sudo systemctl disable getty@tty2.service")
    );
    assert!(!log.iter().any(|call| call.contains("start ly")));
    let config = fs::read_to_string(f.installer.system("/etc/pacman.conf")).unwrap();
    for setting in [
        "Color",
        "VerbosePkgLists",
        "ParallelDownloads = 6",
        "ILoveCandy",
    ] {
        assert!(config.lines().any(|line| line == setting));
    }
}

#[test]
fn required_service_failure_stops_setup_after_enable_fallback() {
    let mut f = Fixture::new();
    f.prepare_system();
    f.override_call(
        "sudo systemctl enable --now NetworkManager.service",
        &[(1, "")],
    );
    let failure = f.installer.setup_system().unwrap_err().to_string();
    assert!(failure.contains("NetworkManager.service"));
    let log = f.log();
    assert!(log.contains(&"sudo systemctl enable NetworkManager.service".into()));
    assert!(
        !log.iter()
            .any(|call| call == "sudo systemctl enable --now power-profiles-daemon.service")
    );
}

#[test]
fn docker_retains_daemon_when_containers_are_running_or_cannot_be_inspected() {
    for (output, retained) in [
        ((0, ""), false),
        ((0, "container-id\n"), true),
        ((1, ""), true),
    ] {
        let mut f = Fixture::new();
        f.prepare_system();
        f.override_call("systemctl is-enabled --quiet docker.service", &[(0, "")]);
        f.override_call(
            "env -u DOCKER_HOST -u DOCKER_CONTEXT docker --host unix:///run/docker.sock ps --quiet",
            &[output],
        );
        f.installer.setup_system().unwrap();
        let log = f.log();
        assert!(
            log.iter()
                .any(|call| call.contains("docker --host unix:///run/docker.sock ps --quiet"))
        );
        assert_eq!(
            log.contains(&"sudo systemctl disable --now docker.service".into()),
            !retained
        );
    }
}

#[test]
fn oomd_requires_effective_limit_and_duration_for_active_user_manager() {
    for (incorrect, expected) in [("3221225471", "1717986918"), ("30s", "10s")] {
        let mut f = Fixture::new();
        f.prepare_oom();
        let uid = unsafe { libc::geteuid() };
        f.override_call(
            &format!("systemctl is-active --quiet user@{uid}.service"),
            &[(0, "")],
        );
        f.override_call(
            &format!("systemctl show user@{uid}.service -p MemoryAccounting -p ManagedOOMMemoryPressure -p ManagedOOMSwap -p ManagedOOMMemoryPressureLimit -p ManagedOOMMemoryPressureDurationUSec"),
            &[(0, &OOM_POLICY.replace(expected, incorrect))],
        );
        assert!(
            f.installer
                .setup_oomd()
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert!(f.installer.system(OOM_DEST).is_file());
        assert!(!f.log().iter().any(|call| call == "oomctl --no-pager"));
    }
}

#[test]
fn lid_policy_is_installed_without_restarting_logind() {
    let mut f = Fixture::new();
    f.installer.setup_logind().unwrap();
    let config = fs::read_to_string(
        f.installer
            .system("/etc/systemd/logind.conf.d/10-hype-niri-lid.conf"),
    )
    .unwrap();
    assert!(config.contains("HandleLidSwitchDocked=ignore"));
    assert!(config.contains("HandleLidSwitch=suspend"));
    assert!(f.log().is_empty());
}

#[test]
fn firewall_preserves_rules_and_adds_hotspot_ports_only_to_ap_capable_wifi() {
    let mut f = Fixture::new();
    f.override_call(
        "nmcli -t -f DEVICE,TYPE device status",
        &[(0, "wlan0:wifi\nwlan1:wifi\neth0:ethernet\n")],
    );
    f.override_call(
        "nmcli -g WIFI-PROPERTIES.AP device show wlan0",
        &[(0, "yes\n")],
    );
    f.override_call(
        "nmcli -g WIFI-PROPERTIES.AP device show wlan1",
        &[(0, "no\n")],
    );
    f.installer.setup_firewall().unwrap();
    let log = f.log();
    assert!(log.contains(&"sudo ufw --force enable".into()));
    assert!(!log.iter().any(|call| call.contains("reset")
        || call.contains("on eth0")
        || call.contains("on wlan1")));
    for (port, protocol) in [(67, "udp"), (53, "udp"), (53, "tcp")] {
        let rule = format!("sudo ufw allow in on wlan0 to any port {port} proto {protocol}");
        assert!(log.iter().any(|call| call.starts_with(&rule)));
    }
}

#[test]
fn warp_fails_open_without_changing_connection_after_registration_or_mode_failure() {
    for (service, register, mode, expected) in [
        (1, 0, 0, [false, false]),
        (0, 1, 0, [true, false]),
        (0, 0, 1, [true, true]),
    ] {
        let mut f = Fixture::new();
        f.override_call(
            "sudo systemctl enable --now warp-svc",
            &[(service, "warp-svc result")],
        );
        f.override_call("warp-cli --accept-tos registration show", &[(1, "")]);
        f.override_call(
            "warp-cli --accept-tos registration new",
            &[(register, "registration result")],
        );
        f.override_call("warp-cli --accept-tos mode doh", &[(mode, "mode result")]);
        f.installer.setup_cloudflare().unwrap();
        let log = f.log();
        assert_eq!(
            log.contains(&"warp-cli --accept-tos registration new".into()),
            expected[0]
        );
        assert_eq!(
            log.contains(&"warp-cli --accept-tos mode doh".into()),
            expected[1]
        );
        assert!(
            !log.iter()
                .any(|call| call == "warp-cli --accept-tos connect"
                    || call == "warp-cli --accept-tos disconnect")
        );
    }
}

#[test]
fn stealth_rejects_busy_current_and_legacy_locks_before_external_calls() {
    for lock in ["/run/stealth.lock", "/run/privacy-shield.lock"] {
        let mut f = Fixture::new();
        let payload = f.prepare_stealth();
        let file = File::create(f.installer.system(lock)).unwrap();
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert!(
            f.installer
                .install_stealth(&payload)
                .unwrap_err()
                .to_string()
                .contains("retry")
        );
        f.assert_legacy_restored();
        assert!(f.log().is_empty());
        assert!(!f.installer.system("/var/backups/stealth").exists());
    }
}

#[test]
fn stealth_rejects_missing_shell_and_incompatible_backends_before_backup() {
    for failure in ["missing", "shell", "version"] {
        let mut f = Fixture::new();
        let payload = f.prepare_stealth();
        let binary = payload.join("stealth");
        match failure {
            "missing" => fs::remove_file(&binary).unwrap(),
            "shell" => put(&binary, "#!/bin/sh\necho stealth-transparent-v2\n"),
            _ => f.override_call("stealth version", &[(0, "incorrect\n")]),
        }
        let result = f
            .installer
            .install_stealth(&payload)
            .unwrap_err()
            .to_string();
        assert!(result.contains(match failure {
            "missing" => "Missing prebuilt",
            "shell" => "native ELF",
            _ => "unsupported version",
        }));
        f.assert_legacy_restored();
        assert!(!f.installer.system("/var/backups/stealth").exists());
    }
}

#[test]
fn stealth_rolls_back_after_live_visudo_or_final_tor_check_fails() {
    for failure in ["visudo", "tor"] {
        let mut f = Fixture::new();
        let payload = f.prepare_stealth();
        if failure == "visudo" {
            f.override_call("visudo -c", &[(1, "")]);
        } else {
            f.override_call(
                &format!(
                    "tor --verify-config -f {}",
                    f.installer.system("/etc/tor/torrc").display()
                ),
                &[(0, ""), (1, "")],
            );
        }
        let result = f
            .installer
            .install_stealth(&payload)
            .unwrap_err()
            .to_string();
        assert!(result.contains("previous files restored"));
        f.assert_legacy_restored();
        assert!(f.installer.system("/var/backups/stealth").is_dir());
        if failure == "tor" {
            let log = f.log();
            let disable = log
                .iter()
                .position(|c| c == "systemctl disable privacy-shield.service")
                .unwrap();
            let enable = log
                .iter()
                .position(|c| c == "systemctl enable privacy-shield.service")
                .unwrap();
            assert!(disable < enable);
        }
    }
}

#[test]
fn stealth_migrates_existing_files_and_reinstalls_without_duplicate_tor_includes() {
    let mut f = Fixture::new();
    let payload = f.prepare_stealth();
    for _ in 0..2 {
        f.installer.install_stealth(&payload).unwrap();
        for (path, mode) in [
            ("/usr/local/bin/stealth", 0o755),
            ("/etc/systemd/system/stealth.service", 0o644),
            ("/etc/stealth.nft", 0o644),
            ("/etc/tor/torrc.d/stealth.conf", 0o644),
            ("/etc/sudoers.d/stealth", 0o440),
            ("/var/log/stealth.log", 0o600),
        ] {
            assert_eq!(
                fs::metadata(f.installer.system(path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                mode
            );
        }
        assert_eq!(
            fs::read(f.installer.system("/usr/local/bin/stealth")).unwrap(),
            fs::read(payload.join("stealth")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(f.installer.system("/var/log/stealth.log")).unwrap(),
            "old log"
        );
        let torrc = fs::read_to_string(f.installer.system("/etc/tor/torrc")).unwrap();
        assert_eq!(
            torrc
                .lines()
                .filter(|line| line.starts_with("%include "))
                .collect::<Vec<_>>(),
            ["%include /etc/tor/torrc.d/stealth.conf"]
        );
        assert!(
            !f.installer
                .system("/usr/local/bin/privacy-shield.sh")
                .exists()
        );
    }
    assert_eq!(
        fs::read_dir(f.installer.system("/var/backups/stealth"))
            .unwrap()
            .count(),
        2
    );
    assert!(
        !f.log()
            .iter()
            .any(|call| call.contains("start stealth.service")
                || call.contains("enable stealth.service"))
    );
}

#[test]
fn validation_checks_required_services_and_live_oom_settings() {
    const CRITICAL: &[&str] = &[
        ".config/niri/config.kdl",
        ".config/niri/opacity.kdl",
        ".config/niri/outputs.kdl",
        ".config/waybar/config.jsonc",
        ".config/waybar/style.css",
        ".config/scripts/hype-notch",
        ".config/scripts/hype-control",
        ".config/scripts/hype-status",
        ".config/scripts/libhype_blueman_keys.so",
        ".config/alacritty/alacritty.toml",
        ".config/nvim/init.lua",
        ".config/nvim/lua/config/lazy.lua",
        ".config/mako/config",
        ".config/fastfetch/config.jsonc",
        ".config/hypr/hyprlock.conf",
        ".config/hypr/hypridle.conf",
        ".config/gtk-3.0/settings.ini",
        ".config/gtk-4.0/settings.ini",
        ".config/fontconfig/conf.d/60-hype-niri-fonts.conf",
        ".config/systemd/user/hype-auto-power-profile.service",
        ".config/systemd/user/hype-sleep-watch.service",
        ".config/systemd/user/hype-clipboard-watch.service",
        ".config/wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf",
        ".zshrc",
        ".p10k.zsh",
    ];
    let mut f = Fixture::new();
    f.prepare_oom();
    fs::create_dir_all(f.installer.system(OOM_DEST).parent().unwrap()).unwrap();
    fs::copy(
        f.installer.source.join(OOM_SOURCE),
        f.installer.system(OOM_DEST),
    )
    .unwrap();
    for path in CRITICAL.iter().map(|path| f.installer.user(path)).chain([f
        .installer
        .system("/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1")])
    {
        put(&path, "");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    f.installer.validate().unwrap();
    // A core installation validates without any bundled applications or retired helpers.
    f.installer.optional_groups = Some(Vec::new());
    f.calls.borrow_mut().unavailable = [
        "nvim",
        "obs",
        "tree-sitter",
        "blueman-applet",
        "wofi",
        "wl-paste",
        "cliphist",
        "playerctl",
        "magick",
        "notify-send",
        "powerprofilesctl",
        "pavucontrol",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for optional in [".config/nvim", ".config/fastfetch"] {
        fs::remove_dir_all(f.installer.user(optional)).unwrap();
    }
    f.installer.validate().unwrap();
    assert!(
        !f.log()
            .iter()
            .any(|call| call.starts_with("systemctl show user@"))
    );
    f.override_call(
        "systemctl list-unit-files NetworkManager.service --no-legend",
        &[(0, "")],
    );
    assert!(f.installer.validate().is_err());
    let uid = unsafe { libc::geteuid() };
    f.override_call(
        &format!("systemctl is-active --quiet user@{uid}.service"),
        &[(0, ""), (0, "")],
    );
    f.override_call(
        &format!("systemctl show user@{uid}.service -p MemoryAccounting -p ManagedOOMMemoryPressure -p ManagedOOMSwap -p ManagedOOMMemoryPressureLimit -p ManagedOOMMemoryPressureDurationUSec"),
        &[(0, &OOM_POLICY.replace("10s", "30s")), (0, OOM_POLICY)],
    );
    assert!(f.installer.validate().is_err());
    f.installer.validate().unwrap();
}

#[test]
fn warp_waits_before_registration_and_respects_skip_connection() {
    let mut f = Fixture::new();
    f.calls.borrow_mut().answer = "3".into();
    f.override_call(
        "warp-cli --accept-tos status",
        &[(1, "not ready"), (0, "ready")],
    );
    f.override_call("warp-cli --accept-tos registration show", &[(1, "")]);
    f.installer.setup_cloudflare().unwrap();
    assert_eq!(
        f.log(),
        [
            "sudo systemctl enable --now warp-svc",
            "warp-cli --accept-tos status",
            "warp-cli --accept-tos status",
            "warp-cli --accept-tos registration show",
            "warp-cli --accept-tos registration new",
        ]
    );
}

#[test]
fn stealth_bad_preview_keeps_existing_installation_before_any_backup() {
    let mut f = Fixture::new();
    let payload = f.prepare_stealth();
    f.calls.borrow_mut().overrides.insert(
        "tor preview".into(),
        VecDeque::from([(1, "syntax error".into())]),
    );
    assert!(f.installer.install_stealth(&payload).is_err());
    assert!(f.log().iter().any(
        |call| call.starts_with("tor --verify-config -f ") && call.ends_with("/torrc.preview")
    ));
    f.assert_legacy_restored();
    assert!(!f.installer.system("/var/backups/stealth").exists());
}

#[test]
fn stealth_publishes_only_rust_payload_after_elevated_install_succeeds() {
    let mut f = Fixture::new();
    let payload = f.prepare_stealth();
    put(
        &f.installer.source.join("zsh/stealth.zsh"),
        "alias stealth-start='hype-control stealth start'\n",
    );
    let binary = f
        .installer
        .user(".cache/hype-niri/cargo-target/x86_64-unknown-linux-gnu/release/stealth");
    put(&binary, fs::read(payload.join("stealth")).unwrap());
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let destination = f.installer.user(".local/share/stealth");
    let legacy = f.installer.user(".local/share/privacy-shield");
    put(&destination.join("old-user-file"), "old payload");
    put(&legacy.join("legacy-user-file"), "old aliases");
    f.override_call("elevated stealth install", &[(1, ""), (0, "")]);
    let failure = f.installer.setup_stealth().unwrap_err().to_string();
    assert!(failure.contains("previous user commands were retained"));
    assert_eq!(
        fs::read_to_string(destination.join("old-user-file")).unwrap(),
        "old payload"
    );
    assert!(legacy.is_dir());

    f.installer.setup_stealth().unwrap();
    let mut names = fs::read_dir(destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        [
            "stealth",
            "stealth.nft",
            "stealth.service",
            "stealth.sudoers",
            "stealth.zsh",
            "torrc.conf"
        ]
    );
    assert!(!legacy.exists());
    assert!(
        f.log()
            .iter()
            .any(|call| call == "sudo pacman -S --needed --noconfirm tor nftables polkit")
    );
}

#[test]
fn core_selection_never_configures_installed_optional_services() {
    let mut f = Fixture::new();
    f.prepare_system();
    f.installer.optional_groups = Some(Vec::new());
    f.installer.setup_system().unwrap();
    f.installer.setup_cloudflare().unwrap();
    f.installer.setup_stealth().unwrap();
    f.installer.setup_firewall().unwrap();
    let calls = &f.calls.borrow().log;
    assert!(
        !calls.iter().any(|call| call.contains("bluetooth")
            || call.contains("docker")
            || call.contains("warp")
            || call.contains("tor")
            || call.contains("ufw")
            || call.contains("ly@")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.contains("NetworkManager.service"))
    );
}
