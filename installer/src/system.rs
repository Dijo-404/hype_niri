use crate::{CommandOutput, Installer, Result, error};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

const SYSTEM_REQUIRED: &[&str] = &["NetworkManager.service", "power-profiles-daemon.service"];

const AUDIO_UNITS: &[&str] = &[
    "pipewire.socket",
    "pipewire-pulse.socket",
    "wireplumber.service",
    "gnome-keyring-daemon.socket",
];
const AUTO_POWER: &str = "hype-auto-power-profile.service";
const OOMD: &str = "systemd-oomd.service";
const OOM_SOURCE: &str = "systemd/user@.service.d/60-hype-niri-oomd.conf";
const OOM_DEST: &str = "/etc/systemd/system/user@.service.d/60-hype-niri-oomd.conf";
const OOM_PROPERTIES: &[(&str, &str)] = &[
    ("MemoryAccounting", "yes"),
    ("ManagedOOMMemoryPressure", "kill"),
    ("ManagedOOMSwap", "kill"),
    ("ManagedOOMMemoryPressureLimit", "1717986918"),
    ("ManagedOOMMemoryPressureDurationUSec", "10s"),
];

const STEALTH_BINARY: &str = "/usr/local/bin/stealth";
const STEALTH_UNIT: &str = "/etc/systemd/system/stealth.service";
const STEALTH_RULES: &str = "/etc/stealth.nft";
const TORRC: &str = "/etc/tor/torrc";
const TOR_DROPIN: &str = "/etc/tor/torrc.d/stealth.conf";
const SUDOERS: &str = "/etc/sudoers.d/stealth";
const STEALTH_LOG: &str = "/var/log/stealth.log";
const OLD_UNIT: &str = "/etc/systemd/system/privacy-shield.service";
const OLD_DROPIN: &str = "/etc/tor/torrc.d/privacy-shield.conf";
const OLD_LOG: &str = "/var/log/privacy-shield.log";
const OLD_LOCK: &str = "/run/privacy-shield.lock";
const LEGACY_FILES: &[(&str, &str)] = &[
    ("/usr/local/bin/privacy-shield.sh", "old-script"),
    (OLD_UNIT, "old-unit"),
    ("/etc/privacy-shield.nft", "old-rules"),
    (OLD_DROPIN, "old-tor-dropin"),
    (OLD_LOG, "old-log"),
];
const STEALTH_FILES: &[(&str, &str)] = &[
    (STEALTH_BINARY, "script"),
    (STEALTH_UNIT, "unit"),
    (STEALTH_RULES, "rules"),
    (TORRC, "torrc"),
    (TOR_DROPIN, "tor-dropin"),
    (SUDOERS, "sudoers"),
    (STEALTH_LOG, "log"),
];
const STEALTH_CONFIG: &[(&str, &str, u32)] = &[
    ("stealth.service", STEALTH_UNIT, 0o644),
    ("stealth.nft", STEALTH_RULES, 0o644),
    ("torrc.conf", TOR_DROPIN, 0o644),
    ("stealth.sudoers", SUDOERS, 0o440),
];

impl Installer {
    fn succeeded(&self, program: &str, args: &[&str]) -> bool {
        self.output(program, args)
            .is_ok_and(|output| output.success())
    }

    fn checked_output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let output = self.output(program, args)?;
        if output.success() {
            Ok(output)
        } else {
            Err(error(format!(
                "{program} {} failed (exit {}): {}",
                args.join(" "),
                output.code,
                output.stderr.trim()
            )))
        }
    }

    fn unit_present(&self, unit: &str, user: bool) -> bool {
        let mut args = Vec::new();
        if user {
            args.push("--user");
        }
        args.extend(["list-unit-files", unit, "--no-legend"]);
        self.output("systemctl", &args)
            .is_ok_and(|output| output.success() && !output.stdout.trim().is_empty())
    }

    fn enable_unit(&self, unit: &str, user: bool, start: bool) -> Result<()> {
        let scope = if user { "user" } else { "system" };
        if !self.unit_present(unit, user) {
            return Err(error(format!("{scope} service unit unavailable: {unit}")));
        }
        let (program, prefix) = if user {
            ("systemctl", "--user")
        } else {
            ("sudo", "systemctl")
        };
        if start && self.succeeded(program, &[prefix, "enable", "--now", unit]) {
            self.note(format_args!("Enabled and started {scope} service: {unit}"));
            return Ok(());
        }
        self.checked_output(program, &[prefix, "enable", unit])?;
        if start {
            return Err(error(format!(
                "Enabled {scope} service but could not start: {unit}"
            )));
        }
        self.note(format_args!("Enabled {scope} service: {unit}"));
        Ok(())
    }

    pub fn setup_system(&mut self) -> Result<()> {
        self.note(format_args!("System configuration"));
        self.configure_pacman()?;

        if self.selected("login-manager-ly") && self.confirm("Set up ly as display manager?")? {
            self.setup_ly()?;
        }

        let mut rules = fs::read_dir(self.source.join("polkit"))
            .map_err(|cause| error(format!("Polkit rules are missing or unreadable: {cause}")))?
            .collect::<io::Result<Vec<_>>>()?;
        rules.retain(|entry| entry.path().extension().is_some_and(|ext| ext == "rules"));
        if rules.is_empty() {
            return Err(error("Polkit rules are missing"));
        }
        rules.sort_by_key(|entry| entry.file_name());
        let rules = rules
            .iter()
            .map(|entry| Ok((entry.file_name(), fs::read(entry.path())?)))
            .collect::<io::Result<Vec<_>>>()?;
        for (name, contents) in rules {
            let path = Path::new("/etc/polkit-1/rules.d").join(&name);
            self.write_system(path_arg(&path)?, &contents, 0o644)?;
        }
        self.note(format_args!("Polkit rules applied"));

        self.setup_oomd()
            .map_err(|cause| error(format!("Could not configure systemd-oomd: {cause}")))?;
        for unit in SYSTEM_REQUIRED {
            self.enable_unit(unit, false, true).map_err(|cause| {
                error(format!(
                    "Required system service could not be configured: {unit}: {cause}"
                ))
            })?;
        }
        if self.selected("bluetooth")
            && let Err(cause) = self.enable_unit("bluetooth.service", false, true)
        {
            self.warn(format_args!("Warning: {cause}"));
        }
        if self.selected("docker") {
            if let Err(cause) = self.configure_docker_socket() {
                self.warn(format_args!(
                    "Warning: could not configure Docker socket activation: {cause}"
                ));
            }
            self.configure_docker_group()?;
        }

        if self.succeeded("systemctl", &["--user", "show-environment"]) {
            for unit in AUDIO_UNITS {
                self.enable_unit(unit, true, true).map_err(|cause| {
                    error(format!(
                        "Required audio service could not be configured: {unit}: {cause}"
                    ))
                })?;
            }
            if let Err(cause) = self.enable_unit("hypridle.service", true, false) {
                self.warn(format_args!(
                    "Warning: {cause}; Niri can launch hypridle directly"
                ));
            }
            self.enable_unit(AUTO_POWER, true, self.graphical_session_active())?;
        } else {
            self.warn(format_args!(
                "Warning: user systemd manager unavailable; audio will use package defaults at login"
            ));
            self.warn(format_args!(
                "Niri startup will launch hypridle with its service or direct fallback"
            ));
        }
        self.note(format_args!("System services configured"));
        Ok(())
    }

    fn configure_pacman(&self) -> Result<()> {
        const CONFIG: &str = "/etc/pacman.conf";
        let path = self.system(CONFIG);
        if !path.is_file() {
            return Ok(());
        }
        let contents = fs::read_to_string(&path)?;
        let mut lines: Vec<_> = contents.lines().collect();
        let options = lines
            .iter()
            .position(|&line| line == "[options]")
            .ok_or_else(|| error("Missing [options] section in /etc/pacman.conf"))?;

        for directive in ["Color", "VerbosePkgLists"] {
            let commented = format!("#{directive}");
            for line in &mut lines {
                if *line == commented {
                    *line = directive;
                }
            }
            if !lines.contains(&directive) {
                lines.insert(options + 1, directive);
            }
        }
        let prefix = if lines
            .iter()
            .any(|line| line.starts_with("#ParallelDownloads"))
        {
            "#ParallelDownloads"
        } else {
            "ParallelDownloads"
        };
        let mut found = false;
        for line in &mut lines {
            if line.starts_with(prefix) {
                *line = "ParallelDownloads = 6";
                found = true;
            }
        }
        if !found {
            lines.insert(options + 1, "ParallelDownloads = 6");
        }
        if !lines.contains(&"ILoveCandy") {
            lines.insert(options + 1, "ILoveCandy");
        }
        let rewritten = lines.join("\n") + "\n";
        if rewritten != contents {
            self.write_system(
                CONFIG,
                rewritten.as_bytes(),
                path.metadata()?.permissions().mode() & 0o7777,
            )?;
        }
        self.note(format_args!("Pacman output tuned"));
        Ok(())
    }

    fn setup_ly(&self) -> Result<()> {
        self.command("sudo", &["systemctl", "daemon-reload"])?;
        let units = self.checked_output(
            "systemctl",
            &["list-unit-files", "--type=service", "--no-legend"],
        )?;
        let units: Vec<_> = units
            .stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let ly = if units.contains(&"ly@.service") {
            "ly@tty2.service"
        } else if units.contains(&"ly.service") {
            "ly.service"
        } else {
            return Err(error(
                "Ly service unit is missing; install ly before switching display managers",
            ));
        };
        self.command("sudo", &["systemctl", "enable", "--force", ly])
            .map_err(|cause| {
                error(format!(
                    "Could not enable {ly}; the current display manager was retained: {cause}"
                ))
            })?;
        for dm in [
            "sddm.service",
            "gdm.service",
            "lightdm.service",
            "greetd.service",
        ] {
            if units.contains(&dm) && self.succeeded("systemctl", &["is-enabled", "--quiet", dm]) {
                self.command("sudo", &["systemctl", "disable", dm]).map_err(|cause| {
                    error(format!("Could not disable {dm}; finish the display manager switch before rebooting: {cause}"))
                })?;
                self.note(format_args!("Disabled {dm}"));
            }
        }
        self.command("sudo", &["systemctl", "disable", "getty@tty2.service"])?;
        self.note(format_args!("Enabled {ly} for the next boot"));
        Ok(())
    }

    pub fn configure_docker_socket(&self) -> Result<()> {
        self.enable_unit("docker.socket", false, true)?;
        if self.succeeded("systemctl", &["is-enabled", "--quiet", "docker.service"]) {
            if self.succeeded("systemctl", &["is-active", "--quiet", "docker.service"]) {
                let containers = self.checked_output(
                    "env",
                    &[
                        "-u",
                        "DOCKER_HOST",
                        "-u",
                        "DOCKER_CONTEXT",
                        "docker",
                        "--host",
                        "unix:///run/docker.sock",
                        "ps",
                        "--quiet",
                    ],
                );
                match containers {
                    Ok(output) if output.stdout.trim().is_empty() => {}
                    Ok(_) => {
                        self.warn(format_args!(
                            "Warning: Docker has running containers; existing service retained"
                        ));
                        return Ok(());
                    }
                    Err(_) => {
                        self.warn(format_args!(
                            "Warning: could not inspect Docker containers; existing service retained"
                        ));
                        return Ok(());
                    }
                }
            }
            self.command("sudo", &["systemctl", "disable", "--now", "docker.service"])?;
        }
        self.note(format_args!("Docker starts on demand through its socket"));
        Ok(())
    }

    fn configure_docker_group(&self) -> Result<()> {
        if !self.runner.available("docker") || !self.succeeded("getent", &["group", "docker"]) {
            return Ok(());
        }
        let user = self.checked_output("id", &["-un"])?;
        let user = user.stdout.trim();
        if self.output("id", &["-nG", user]).is_ok_and(|groups| {
            groups.success()
                && groups
                    .stdout
                    .split_whitespace()
                    .any(|group| group == "docker")
        }) {
            self.note(format_args!("User {user} is already in docker group"));
        } else if self.confirm(&format!(
            "Add {user} to docker group? This requires logging out and back in."
        ))? {
            match self.command("sudo", &["usermod", "-aG", "docker", user]) {
                Ok(()) => self.note(format_args!(
                    "Added {user} to docker group. Log out and back in before running Docker without sudo."
                )),
                Err(cause) => self.warn(format_args!("Warning: could not add {user} to docker group: {cause}")),
            }
        } else {
            self.warn(format_args!(
                "Warning: Docker group setup skipped; use sudo docker or add your user later"
            ));
        }
        Ok(())
    }

    fn graphical_session_active(&self) -> bool {
        self.succeeded(
            "systemctl",
            &["--user", "is-active", "--quiet", "graphical-session.target"],
        )
    }

    pub fn setup_oomd(&mut self) -> Result<()> {
        let version = self.checked_output("systemctl", &["--version"])?;
        if !version
            .stdout
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<u32>().ok())
            .is_some_and(|v| v >= 257)
        {
            return Err(error("OOM protection requires systemd 257 or newer"));
        }
        let controllers = fs::read_to_string(self.system("/sys/fs/cgroup/cgroup.controllers"));
        if !controllers
            .is_ok_and(|contents| contents.split_whitespace().any(|name| name == "memory"))
            || File::open(self.system("/proc/pressure/memory")).is_err()
        {
            return Err(error(
                "OOM protection requires cgroup v2, the memory controller, and PSI",
            ));
        }
        let contents = fs::read(self.source.join(OOM_SOURCE))?;
        self.write_system(OOM_DEST, &contents, 0o644)?;
        if self.root == Path::new("/") && effective_uid() == 0 {
            self.command("systemctl", &["daemon-reload"])?;
            self.command("systemctl", &["enable", "--now", OOMD])?;
        } else {
            self.command("sudo", &["systemctl", "daemon-reload"])?;
            self.command("sudo", &["systemctl", "enable", "--now", OOMD])?;
        }
        self.checked_output("systemctl", &["is-enabled", "--quiet", OOMD])?;
        self.checked_output("systemctl", &["is-active", "--quiet", OOMD])?;
        self.check_oom_policy()?;
        self.note(format_args!(
            "systemd-oomd enabled and running. Monitored groups:"
        ));
        self.command("oomctl", &["--no-pager"])
    }

    fn check_oom_policy(&self) -> Result<bool> {
        let uid = if effective_uid() == 0 {
            std::env::var("SUDO_UID")
                .ok()
                .and_then(|uid| uid.parse::<u32>().ok())
                .unwrap_or(0)
        } else {
            effective_uid()
        };
        let manager = format!("user@{uid}.service");
        if !self.succeeded("systemctl", &["is-active", "--quiet", &manager]) {
            return Ok(false);
        }
        let mut args = vec!["show", &manager];
        for (name, _) in OOM_PROPERTIES {
            args.extend(["-p", name]);
        }
        let policy = self.checked_output("systemctl", &args)?;
        for (name, value) in OOM_PROPERTIES {
            let setting = format!("{name}={value}");
            if !policy.stdout.lines().any(|line| line == setting) {
                return Err(error(format!(
                    "OOM policy not active on {manager}: expected {setting}. Check other systemd drop-ins."
                )));
            }
        }
        Ok(true)
    }

    pub fn setup_logind(&mut self) -> Result<()> {
        if !self.confirm("Configure systemd-logind to suspend on lid close (but keep running when an external monitor is connected)?")? {
            self.note(format_args!("Lid switch setup skipped"));
            return Ok(());
        }
        const PATH: &str = "/etc/systemd/logind.conf.d/10-hype-niri-lid.conf";
        const CONTENTS: &[u8] = b"[Login]\nHandleLidSwitch=suspend\nHandleLidSwitchExternalPower=suspend\nHandleLidSwitchDocked=ignore\nLidSwitchIgnoreInhibited=yes\nHoldoffTimeoutSec=0s\nInhibitDelayMaxSec=5\n";
        self.write_system(PATH, CONTENTS, 0o644)?;
        self.note(format_args!(
            "Wrote {PATH}. Lid switch changes will apply after reboot."
        ));
        Ok(())
    }

    pub fn setup_firewall(&mut self) -> Result<()> {
        if !self.selected("firewall") {
            return Ok(());
        }
        if !self.runner.available("ufw") {
            self.note(format_args!("ufw is unavailable; firewall setup skipped"));
            return Ok(());
        }
        if !self.confirm("Configure ufw with desktop defaults (deny incoming, allow outgoing)?")? {
            self.note(format_args!(
                "Firewall setup skipped. You can run 'sudo ufw enable' later."
            ));
            return Ok(());
        }
        for args in [
            ["ufw", "default", "deny", "incoming"].as_slice(),
            &["ufw", "default", "allow", "outgoing"],
            &["ufw", "default", "allow", "routed"],
            &["ufw", "allow", "in", "on", "lo"],
            &["ufw", "allow", "out", "on", "lo"],
        ] {
            self.checked_output("sudo", args)?;
        }
        if self.runner.available("nmcli")
            && let Ok(devices) =
                self.checked_output("nmcli", &["-t", "-f", "DEVICE,TYPE", "device", "status"])
        {
            for line in devices.stdout.lines() {
                let Some((device, "wifi")) = line.rsplit_once(':') else {
                    continue;
                };
                let device = device.replace("\\\\", "\\");
                let capability = self.checked_output(
                    "nmcli",
                    &["-g", "WIFI-PROPERTIES.AP", "device", "show", &device],
                );
                if capability.is_ok_and(|output| output.stdout.trim() == "yes") {
                    for (port, protocol, comment) in [
                        ("67", "udp", "Hype hotspot DHCP"),
                        ("53", "udp", "Hype hotspot DNS"),
                        ("53", "tcp", "Hype hotspot DNS"),
                    ] {
                        self.checked_output(
                            "sudo",
                            &[
                                "ufw", "allow", "in", "on", &device, "to", "any", "port", port,
                                "proto", protocol, "comment", comment,
                            ],
                        )?;
                    }
                }
            }
        }
        self.checked_output("sudo", &["ufw", "logging", "low"])?;
        self.checked_output("sudo", &["ufw", "--force", "enable"])?;
        self.command("sudo", &["systemctl", "enable", "ufw.service"])?;
        self.note(format_args!(
            "ufw enabled with desktop defaults. Current rules:"
        ));
        for line in self
            .checked_output("sudo", &["ufw", "status", "verbose"])?
            .stdout
            .lines()
        {
            self.note(format_args!("    {line}"));
        }
        Ok(())
    }

    pub fn setup_cloudflare(&mut self) -> Result<()> {
        if !self.selected("warp") {
            return Ok(());
        }
        if !self.runner.available("warp-cli") {
            self.note(format_args!(
                "warp-cli is unavailable; install cloudflare-warp-bin to use Cloudflare WARP"
            ));
            return Ok(());
        }
        if !self
            .confirm("Set up Cloudflare WARP (DNS-over-HTTPS by default, full VPN optional)?")?
        {
            self.note(format_args!("Cloudflare WARP setup skipped"));
            return Ok(());
        }

        if let Err(cause) =
            warp_result(self.output("sudo", &["systemctl", "enable", "--now", "warp-svc"]))
        {
            warn_warp(self, "Could not enable and start warp-svc", &cause);
            return Ok(());
        }
        self.note(format_args!("warp-svc enabled and running"));
        let status = ["--accept-tos", "status"];
        for attempt in 1..=10 {
            match warp_result(self.output("warp-cli", &status)) {
                Ok(_) => break,
                Err(cause) if attempt == 10 => {
                    warn_warp(
                        self,
                        "WARP daemon did not become ready for CLI requests",
                        &cause,
                    );
                    return Ok(());
                }
                Err(_) => thread::sleep(Duration::from_secs(1)),
            }
        }
        if !self.succeeded("warp-cli", &["--accept-tos", "registration", "show"]) {
            if let Err(cause) =
                warp_result(self.output("warp-cli", &["--accept-tos", "registration", "new"]))
            {
                warn_warp(
                    self,
                    "WARP registration failed; mode and connection were preserved",
                    &cause,
                );
                self.warn(format_args!(
                    "Manually retry with: warp-cli --accept-tos registration new"
                ));
                return Ok(());
            }
            self.note(format_args!("Device registered with Cloudflare"));
        }
        self.note(format_args!(
            "Choose WARP mode:\n  1) DNS-over-HTTPS only [default]\n  2) Full WARP VPN (encrypted tunnel)\n  3) Preserve the current mode and connection"
        ));
        let selection = self.runner.input("Mode [1/2/3]: ")?;
        let (mode, description) = match selection.trim() {
            "1" | "" => ("doh", "DoH"),
            "2" => ("warp", "WARP (VPN)"),
            "3" => {
                self.note(format_args!("WARP mode and connection preserved"));
                return Ok(());
            }
            _ => {
                warn_warp(self, "Invalid WARP mode; existing connection preserved", "");
                return Ok(());
            }
        };
        if let Err(cause) = warp_result(self.output("warp-cli", &["--accept-tos", "mode", mode])) {
            warn_warp(
                self,
                &format!("Failed to set {description} mode; existing connection preserved"),
                &cause,
            );
            return Ok(());
        }
        self.note(format_args!("WARP mode: {description}"));
        if let Err(cause) = warp_result(self.output("warp-cli", &["--accept-tos", "connect"])) {
            warn_warp(
                self,
                "WARP connection failed; check 'warp-cli status' manually",
                &cause,
            );
        } else {
            thread::sleep(Duration::from_secs(1));
            let status = self
                .checked_output("warp-cli", &status)
                .map_or_else(|_| "unknown".into(), |output| output.stdout);
            self.note(format_args!("WARP: {}", status.trim()));
        }
        Ok(())
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn warp_result(output: Result<CommandOutput>) -> std::result::Result<(), String> {
    let output = output.map_err(|cause| cause.to_string())?;
    if output.success() {
        Ok(())
    } else {
        Err(format!(
            "{}\n{}",
            output.stdout.trim(),
            output.stderr.trim()
        ))
    }
}

fn warn_warp(installer: &Installer, message: &str, diagnostics: &str) {
    installer.warn(format_args!("Warning: {message}"));
    for line in diagnostics.trim().lines().take(20) {
        installer.warn(format_args!("    {line}"));
    }
    installer.warn(format_args!(
        "Cloudflare WARP setup incomplete; continuing installation"
    ));
    installer.warn(format_args!(
        "Inspect WARP logs with: journalctl -u warp-svc"
    ));
}

fn path_arg(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| error(format!("Path is not valid UTF-8: {}", path.display())))
}

impl Installer {
    pub fn validate(&mut self) -> Result<()> {
        self.note(format_args!("Validating installation"));
        let required_commands = [
            "cc",
            "pkg-config",
            "niri",
            "xdg-terminal-exec",
            "waybar",
            "hyprlock",
            "hypridle",
            "mako",
            "gnome-keyring-daemon",
            "nm-applet",
            "nmcli",
            "dnsmasq",
            "loginctl",
        ];
        let critical_files = [
            ".config/niri/config.kdl",
            ".config/niri/opacity.kdl",
            ".config/niri/outputs.kdl",
            ".config/waybar/config.jsonc",
            ".config/waybar/style.css",
            ".config/scripts/hype-notch",
            ".config/scripts/hype-control",
            ".config/scripts/hype-status",
            ".config/alacritty/alacritty.toml",
            ".config/mako/config",
            ".config/hypr/hyprlock.conf",
            ".config/hypr/hypridle.conf",
            ".config/gtk-3.0/settings.ini",
            ".config/gtk-4.0/settings.ini",
            ".config/fontconfig/conf.d/60-hype-niri-fonts.conf",
            ".config/systemd/user/hype-auto-power-profile.service",
            ".config/wireplumber/wireplumber.conf.d/60-hype-niri-audio.conf",
            ".zshrc",
            ".p10k.zsh",
        ];
        let mut all_ok = true;
        for program in required_commands {
            if self.runner.available(program) {
                self.note(format_args!("Command available: {program}"));
            } else {
                self.warn(format_args!("Missing command: {program}"));
                all_ok = false;
            }
        }
        let optional_commands = [
            ("editor-neovim", "nvim"),
            ("editor-neovim", "tree-sitter"),
            ("bluetooth", "blueman-applet"),
            ("docker", "docker"),
            ("warp", "warp-cli"),
            ("browser-firefox", "firefox"),
            ("browser-brave", "brave"),
            ("editor-kate", "kate"),
            ("editor-gnome", "gnome-text-editor"),
            ("editor-code", "code"),
            ("chat", "discord"),
            ("notes", "obsidian"),
            ("media", "obs"),
            ("hype-flow", "wl-copy"),
            ("hype-flow", "wtype"),
            ("hype-flow", "bwrap"),
            ("desktop-tools", "pavucontrol"),
            ("desktop-tools", "hyprpicker"),
            ("desktop-tools", "htop"),
            ("firewall", "ufw"),
        ];
        for (group, program) in optional_commands {
            if self.selected(group) && !self.runner.available(program) {
                self.warn(format_args!("Missing selected {group} command: {program}"));
                all_ok = false;
            }
        }
        if self.selected("hype-flow") {
            let flow = self.user(".local/bin/hype-flow");
            if !executable(&flow) || !self.user(".config/niri/hype-flow.kdl").is_file() {
                self.warn(format_args!(
                    "Hype Flow installation or Niri shortcuts are missing"
                ));
                all_ok = false;
            }
        }
        if self.selected("editor-neovim") {
            for file in [".config/nvim/init.lua", ".config/nvim/lua/config/lazy.lua"] {
                if !self.user(file).is_file() {
                    all_ok = false;
                    self.warn(format_args!("Missing {file}"));
                }
            }
        }
        const POLKIT_AGENT: &str = "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1";
        if executable(&self.system(POLKIT_AGENT)) {
            self.note(format_args!("Polkit agent executable present"));
        } else {
            self.warn(format_args!("Missing polkit agent: {POLKIT_AGENT}"));
            all_ok = false;
        }

        if self.runner.available("niri") {
            let config = self.user(".config/niri/config.kdl");
            if self
                .command("niri", &["validate", "-c", path_arg(&config)?])
                .is_ok()
            {
                self.note(format_args!("Niri config is valid"));
            } else {
                self.warn(format_args!(
                    "Niri config validation failed; check {}",
                    config.display()
                ));
                all_ok = false;
            }
        }
        for relative in critical_files {
            let file = self.user(relative);
            if file.is_file() {
                self.note(format_args!("Found {relative}"));
            } else {
                self.warn(format_args!("Missing: {}", file.display()));
                all_ok = false;
            }
        }
        for name in crate::config::DESKTOP_PROGRAMS {
            let path = self.user(&format!(".config/scripts/{name}"));
            if executable(&path) && self.command(path_arg(&path)?, &["--check"]).is_ok() {
                self.note(format_args!("{name} runtime available"));
            } else {
                self.warn(format_args!(
                    "{name} runtime check failed; verify the installed binary and GTK dependencies"
                ));
                all_ok = false;
            }
        }

        self.validate_fonts();
        for (unit, required) in SYSTEM_REQUIRED.iter().map(|unit| (*unit, true)).chain(
            [
                ("bluetooth", "bluetooth.service"),
                ("docker", "docker.socket"),
            ]
            .into_iter()
            .filter(|(group, _)| self.selected(group))
            .map(|(_, unit)| (unit, self.optional_groups.is_some())),
        ) {
            all_ok &= self.validate_unit(unit, false, required, true);
        }
        if self.succeeded("systemctl", &["is-enabled", "--quiet", OOMD])
            && self.succeeded("systemctl", &["is-active", "--quiet", OOMD])
        {
            self.note(format_args!("systemd-oomd enabled and running"));
        } else {
            self.warn(format_args!("systemd-oomd must be enabled and running"));
            all_ok = false;
        }
        match (
            fs::read(self.source.join(OOM_SOURCE)),
            fs::read(self.system(OOM_DEST)),
        ) {
            (Ok(source), Ok(installed)) if source == installed => {
                self.note(format_args!(
                    "User memory pressure protection policy installed"
                ));
            }
            _ => {
                self.warn(format_args!(
                    "User memory pressure protection policy is missing or outdated"
                ));
                all_ok = false;
            }
        }
        match self.check_oom_policy() {
            Ok(true) => self.note(format_args!(
                "User memory pressure and swap monitoring enabled"
            )),
            Ok(false) => self.warn(format_args!(
                "Warning: user manager inactive; live OOM monitoring will apply at login"
            )),
            Err(cause) => {
                self.warn(format_args!(
                    "User OOM monitoring differs from the required 40% for 10 seconds: {cause}"
                ));
                all_ok = false;
            }
        }

        if self.succeeded("systemctl", &["--user", "show-environment"]) {
            for unit in AUDIO_UNITS {
                all_ok &= self.validate_unit(unit, true, true, true);
            }
            self.validate_unit("hypridle.service", true, false, false);
            all_ok &= self.validate_unit(AUTO_POWER, true, true, self.graphical_session_active());
        } else {
            self.warn(format_args!(
                "Warning: user systemd manager unavailable; skipping live user-service validation"
            ));
        }
        if all_ok {
            self.note(format_args!("All files in place!"));
            Ok(())
        } else {
            Err(error(
                "Validation failed; installation did not complete cleanly",
            ))
        }
    }

    fn validate_fonts(&self) {
        if !self.runner.available("fc-match") {
            self.warn(format_args!(
                "Warning: fontconfig is unavailable; skipping Waybar font validation"
            ));
            return;
        }
        for (family, package) in [
            ("JetBrains Mono", "ttf-jetbrains-mono"),
            ("Roboto", "ttf-roboto"),
            ("Material Design Icons", "ttf-material-design-icons-webfont"),
        ] {
            let matched = self.output("fc-match", &["-f", "%{family}\\n", family]);
            if matched.is_ok_and(|output| {
                output.success()
                    && output
                        .stdout
                        .lines()
                        .next()
                        .is_some_and(|line| line.contains(family))
            }) {
                self.note(format_args!("Font: {family}"));
            } else {
                self.warn(format_args!(
                    "Warning: {family} font is unavailable; install {package}"
                ));
            }
        }
    }

    fn validate_unit(&self, unit: &str, user: bool, required: bool, active: bool) -> bool {
        let scope = if user { "user" } else { "system" };
        let severity = if required { "Required" } else { "Optional" };
        if !self.unit_present(unit, user) {
            self.warn(format_args!(
                "{severity} {scope} service unit unavailable: {unit}"
            ));
            return !required;
        }
        let status = |operation| {
            let mut args = vec![operation, "--quiet", unit];
            if user {
                args.insert(0, "--user");
            }
            self.succeeded("systemctl", &args)
        };
        let enabled = status("is-enabled");
        if enabled {
            self.note(format_args!("{scope} service enabled: {unit}"));
        } else {
            self.warn(format_args!(
                "{severity} {scope} service is disabled: {unit}"
            ));
        }
        let running = !active || status("is-active");
        if active {
            if running {
                self.note(format_args!("{scope} service running: {unit}"));
            } else {
                self.warn(format_args!(
                    "{severity} {scope} service is inactive: {unit}"
                ));
            }
        }
        !required || (enabled && running)
    }

    pub fn setup_stealth(&mut self) -> Result<()> {
        if !self.selected("stealth") {
            return Ok(());
        }
        if !self.confirm("Install Stealth routing? It stays off until stealth-start and supports NetworkManager Wi-Fi or Ethernet.")? {
            self.note(format_args!("Stealth installation skipped"));
            return Ok(());
        }
        let aliases = self.source.join("zsh/stealth.zsh");
        let assets = self.source.join("stealth");
        if !aliases.is_file()
            || STEALTH_CONFIG
                .iter()
                .any(|(name, _, _)| !assets.join(name).is_file())
        {
            return Err(error(format!(
                "Stealth files are missing from {}",
                self.source.display()
            )));
        }
        self.command(
            "sudo",
            &[
                "pacman",
                "-S",
                "--needed",
                "--noconfirm",
                "tor",
                "nftables",
                "polkit",
            ],
        )?;

        let build = self.build_stealth()?;
        let binary = build.path().join("stealth");
        let share = self.user(".local/share");
        fs::create_dir_all(&share)?;
        let temporary = tempfile::Builder::new()
            .prefix(".stealth.")
            .tempdir_in(&share)?;
        let staged = temporary.path().join("payload");
        fs::create_dir(&staged)?;
        fs::copy(aliases, staged.join("stealth.zsh"))?;
        for (name, _, _) in STEALTH_CONFIG {
            fs::copy(assets.join(name), staged.join(name))?;
        }
        let staged_binary = staged.join("stealth");
        fs::copy(binary, &staged_binary)?;
        fs::set_permissions(&staged_binary, fs::Permissions::from_mode(0o755))?;

        let installer = std::env::current_exe()?;
        self.command(
            "sudo",
            &[
                "--",
                path_arg(&installer)?,
                "--install-stealth",
                path_arg(&staged)?,
            ],
        )
        .map_err(|cause| {
            error(format!(
                "Stealth installation failed; previous user commands were retained: {cause}"
            ))
        })?;
        crate::config::replace_directory(&staged, &share.join("stealth"))?;
        let previous = share.join("privacy-shield");
        if path_exists(&previous)? {
            if fs::symlink_metadata(&previous)?.is_dir() {
                fs::remove_dir_all(previous)?;
            } else {
                fs::remove_file(previous)?;
            }
        }
        self.note(format_args!(
            "Stealth service installed and inactive. Open a new terminal to use stealth-start, stealth-status, or stealth-stop."
        ));
        Ok(())
    }

    pub fn install_stealth(&mut self, payload: &Path) -> Result<()> {
        if self.root == Path::new("/") && effective_uid() != 0 {
            return Err(error("Run Stealth installation with sudo"));
        }
        for command in [
            "nft",
            "tor",
            "systemd-analyze",
            "systemctl",
            "visudo",
            "pkexec",
            "cp",
        ] {
            if !self.runner.available(command) {
                return Err(error(format!("Missing command: {command}")));
            }
        }
        let _lock = lock_stealth(
            &self.system("/run/stealth.lock"),
            true,
            "Another Stealth operation is in progress; retry after it finishes.",
        )?;
        let old_lock_path = self.system(OLD_LOCK);
        let old_lock = if path_exists(&old_lock_path)? {
            Some(lock_stealth(
                &old_lock_path,
                false,
                "The previous backend is changing state; retry after it finishes.",
            )?)
        } else {
            None
        };

        let stage = tempfile::tempdir()?;
        let binary = payload.join("stealth");
        let staged_binary = stage.path().join("stealth");
        let mut file = open_regular(&binary).map_err(|cause| {
            error(format!(
                "Missing prebuilt native Stealth binary: {}: {cause}",
                binary.display()
            ))
        })?;
        if file.metadata()?.permissions().mode() & 0o111 == 0 {
            return Err(error(format!(
                "Missing prebuilt native Stealth binary: {}",
                binary.display()
            )));
        }
        let mut magic = [0; 4];
        if file.read_exact(&mut magic).is_err() || &magic != b"\x7fELF" {
            return Err(error(
                "Stealth backend must be a prebuilt native ELF executable.",
            ));
        }
        let mut snapshot = magic.to_vec();
        file.read_to_end(&mut snapshot)?;
        fs::write(&staged_binary, &snapshot)?;
        fs::set_permissions(&staged_binary, fs::Permissions::from_mode(0o700))?;
        if !self
            .output(path_arg(&staged_binary)?, &["version"])
            .is_ok_and(|output| {
                output.success() && output.stdout.trim_end_matches('\n') == "stealth-transparent-v2"
            })
        {
            return Err(error(
                "Native Stealth binary has an unsupported version or cannot run.",
            ));
        }

        let torrc_path = self.system(TORRC);
        if !torrc_path.is_file() {
            return Err(error(format!(
                "Missing Tor configuration: {}",
                torrc_path.display()
            )));
        }
        self.check_stealth_inactive()?;

        // Snapshot the unprivileged payload before validating or publishing its contents.
        for (name, _, _) in STEALTH_CONFIG {
            let mut bytes = Vec::new();
            open_regular(&payload.join(name))?.read_to_end(&mut bytes)?;
            fs::write(stage.path().join(name), bytes)?;
        }
        let torrc = fs::read_to_string(&torrc_path)?;
        let base = without_stealth_includes(&torrc);
        let preview = stage.path().join("torrc.preview");
        let mut preview_contents = base.as_bytes().to_vec();
        preview_contents.extend(fs::read(stage.path().join("torrc.conf"))?);
        fs::write(&preview, preview_contents)?;
        self.command("tor", &["--verify-config", "-f", path_arg(&preview)?])?;
        self.command(
            "nft",
            &[
                "-c",
                "-D",
                "uplink=\"wlan0\"",
                "-f",
                path_arg(&stage.path().join("stealth.nft"))?,
            ],
        )?;
        self.command(
            "visudo",
            &["-cqf", path_arg(&stage.path().join("stealth.sudoers"))?],
        )?;

        let backups = self.system("/var/backups/stealth");
        create_directory(&backups, 0o700)?;
        let backup = tempfile::Builder::new()
            .prefix("install.")
            .tempdir_in(&backups)?
            .keep();
        let torrc_backup = self.backup_stealth_files(&backup)?;

        let old_unit_enabled = self.system(OLD_UNIT).exists()
            && self.succeeded(
                "systemctl",
                &["is-enabled", "--quiet", "privacy-shield.service"],
            );
        let rewritten = format!("{}\n\n%include {TOR_DROPIN}\n", base.trim_end_matches('\n'));
        let install = self
            .publish_stealth(stage.path(), &snapshot, &rewritten, old_unit_enabled)
            .and_then(|()| {
                if old_lock.is_some() {
                    fs::remove_file(old_lock_path)?;
                }
                Ok(())
            });
        if let Err(cause) = install {
            let rollback = self.restore_stealth_files(&backup, &torrc_backup, old_unit_enabled);
            return Err(error(match rollback {
                Ok(()) => format!(
                    "Stealth installation failed; previous files restored from {}: {cause}",
                    backup.display()
                ),
                Err(rollback) => format!(
                    "Stealth installation failed: {cause}. Rollback incomplete: {rollback}. Recovery backup: {}",
                    backup.display()
                ),
            }));
        }
        let _ = fs::remove_dir(self.system("/run/privacy-shield"));
        self.note(format_args!(
            "Installed Stealth backend. Backup: {}",
            backup.display()
        ));
        self.note(format_args!(
            "Service is inactive. Use stealth-start, stealth-status, and stealth-stop in a new terminal."
        ));
        self.note(format_args!(
            "Stealth requires a NetworkManager Wi-Fi or Ethernet default route and a Tor service account named tor."
        ));
        Ok(())
    }

    fn check_stealth_inactive(&self) -> Result<()> {
        for service in ["stealth.service", "privacy-shield.service", "tor.service"] {
            if self.succeeded("systemctl", &["is-active", "--quiet", service]) {
                return Err(error(format!("Stop {service} before installing Stealth.")));
            }
        }
        for table in ["stealth", "privacy_shield"] {
            if self.succeeded("nft", &["list", "table", "inet", table]) {
                return Err(error(format!(
                    "The {table} firewall is active; stop it before installing."
                )));
            }
        }
        for path in ["/run/stealth/state", "/run/privacy-shield/state"] {
            let path = self.system(path);
            if path_exists(&path)? {
                return Err(error(format!(
                    "Saved state remains at {}; recover the service before installing.",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    fn backup_stealth_files(&self, backup: &Path) -> Result<PathBuf> {
        for (path, name) in STEALTH_FILES.iter().chain(LEGACY_FILES) {
            let source = self.system(path);
            if path_exists(&source)? {
                self.command(
                    "cp",
                    &[
                        "-a",
                        "--",
                        path_arg(&source)?,
                        path_arg(&backup.join(name))?,
                    ],
                )?;
            } else {
                fs::write(backup.join(format!("{name}.absent")), b"")?;
            }
        }
        let torrc = self.system(TORRC);
        if fs::symlink_metadata(&torrc)?.is_symlink() {
            let target = backup.join("torrc-target");
            self.command("cp", &["-aL", "--", path_arg(&torrc)?, path_arg(&target)?])?;
            Ok(target)
        } else {
            Ok(backup.join("torrc"))
        }
    }

    fn restore_stealth_files(
        &self,
        backup: &Path,
        torrc: &Path,
        old_unit_enabled: bool,
    ) -> Result<()> {
        let mut errors = Vec::new();
        for (path, name) in STEALTH_FILES.iter().chain(LEGACY_FILES) {
            let restore = || -> Result<()> {
                let destination = self.system(path);
                if *path == TORRC {
                    // Tor's administrator-owned file or symlink keeps its identity and permissions.
                    fs::write(destination, fs::read(torrc)?)?;
                } else {
                    remove_file_if_present(&destination)?;
                    if !backup.join(format!("{name}.absent")).exists() {
                        self.command(
                            "cp",
                            &[
                                "-a",
                                "--",
                                path_arg(&backup.join(name))?,
                                path_arg(&destination)?,
                            ],
                        )?;
                    }
                }
                Ok(())
            };
            if let Err(cause) = restore() {
                errors.push(format!("{path}: {cause}"));
            }
        }
        if let Err(cause) = self.command("systemctl", &["daemon-reload"]) {
            errors.push(cause.to_string());
        }
        if old_unit_enabled
            && let Err(cause) = self.command("systemctl", &["enable", "privacy-shield.service"])
        {
            errors.push(cause.to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(error(errors.join(", ")))
        }
    }

    fn publish_stealth(
        &self,
        stage: &Path,
        binary: &[u8],
        rewritten_torrc: &str,
        old_unit_enabled: bool,
    ) -> Result<()> {
        self.write_system(STEALTH_BINARY, binary, 0o755)?;
        create_directory(&self.system("/etc/tor/torrc.d"), 0o755)?;
        create_directory(&self.system("/etc/sudoers.d"), 0o750)?;
        for (name, path, mode) in STEALTH_CONFIG {
            self.write_system(path, &fs::read(stage.join(name))?, *mode)?;
        }
        let sudoers = self.system(SUDOERS);
        let report = self.checked_output("visudo", &["-c"])?;
        if !report.stdout.contains(&format!("{}:", sudoers.display())) {
            self.warn(format_args!(
                "Warning: /etc/sudoers does not include {SUDOERS}; sudo keeps its pty for Stealth."
            ));
        }

        let torrc = self.system(TORRC);
        fs::write(&torrc, rewritten_torrc)?;
        let verify_tor = ["--verify-config", "-f", path_arg(&torrc)?];
        self.command("tor", &verify_tor)?;
        self.command(
            "systemd-analyze",
            &["verify", path_arg(&self.system(STEALTH_UNIT))?],
        )?;
        if old_unit_enabled {
            self.command("systemctl", &["disable", "privacy-shield.service"])?;
        }
        self.command("systemctl", &["daemon-reload"])?;

        for (path, _) in LEGACY_FILES {
            if *path != OLD_LOG {
                remove_file_if_present(&self.system(path))?;
            }
        }
        let old_log = self.system(OLD_LOG);
        let log = self.system(STEALTH_LOG);
        if old_log.exists() {
            if log.exists() {
                fs::remove_file(old_log)?;
            } else {
                fs::rename(old_log, &log)?;
            }
        }
        if log.exists() {
            fs::set_permissions(log, fs::Permissions::from_mode(0o600))?;
        }
        self.command("tor", &verify_tor)?;
        self.command("systemctl", &["daemon-reload"])
    }
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn path_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(cause) => Err(cause),
    }
}

fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(format!(
            "Expected a regular file: {}",
            path.display()
        )));
    }
    Ok(file)
}

struct StealthLock(File);
impl Drop for StealthLock {
    fn drop(&mut self) {
        // An unrelated child fork may briefly retain the fd before exec closes it.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn lock_stealth(path: &Path, create: bool, conflict: &str) -> Result<StealthLock> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != effective_uid() {
        return Err(error(format!(
            "Unsafe Stealth lock file: {}",
            path.display()
        )));
    }
    // SAFETY: file owns its open fd until the installer returns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let cause = io::Error::last_os_error();
        return Err(error(format!("{conflict} ({cause})")));
    }
    Ok(StealthLock(file))
}

fn create_directory(path: &Path, mode: u32) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn without_stealth_includes(torrc: &str) -> String {
    let include = format!("%include {TOR_DROPIN}");
    let old_include = format!("%include {OLD_DROPIN}");
    let mut base = String::new();
    for line in torrc.split_terminator('\n') {
        if line != include && line != old_include {
            base.push_str(line);
            base.push('\n');
        }
    }
    base
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    #[test]
    fn completed_operation_releases_lock_even_if_a_child_inherited_a_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lock");
        let lock = lock_stealth(&path, true, "busy").unwrap();
        let inherited = lock.0.try_clone().unwrap();
        drop(lock);
        assert!(lock_stealth(&path, true, "busy").is_ok());
        drop(inherited);
    }
}
