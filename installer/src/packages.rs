use crate::{Installer, Result, error};
use std::{collections::HashSet, fs, io::Write};

impl Installer {
    pub fn optional_package_groups(&self) -> Result<Vec<(String, Vec<String>)>> {
        optional_groups(&fs::read_to_string(
            self.source.join("optional-packages.txt"),
        )?)
    }

    pub fn saved_optional_groups(&self) -> Result<Option<Vec<String>>> {
        match fs::read_to_string(self.user(".config/hype-niri/optional-groups")) {
            Ok(contents) => Ok(Some(
                contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(cause) => Err(cause.into()),
        }
    }

    pub fn select_optional_groups(&mut self) -> Result<()> {
        if self.optional_groups.is_some() {
            return Ok(());
        }
        let groups = self.optional_package_groups()?;
        println!("  Enter / y  Full desktop with all {} extras", groups.len());
        println!("  n          Core desktop only");
        println!("  custom     Choose extras by group\n");
        let selected = loop {
            let answer = self.runner.input("Install extras? [Y/n/custom] ")?;
            match answer.to_ascii_lowercase().as_str() {
                "" | "y" | "yes" => break groups.iter().map(|(name, _)| name.clone()).collect(),
                "n" | "no" => break Vec::new(),
                "custom" => break self.choose_groups(&groups)?,
                _ => eprintln!("  Please answer y, n, or custom."),
            }
        };
        if selected.is_empty() {
            println!("  Selected core desktop only.");
        } else {
            println!(
                "  Selected {} of {} extra groups.",
                selected.len(),
                groups.len()
            );
        }
        self.optional_groups = Some(selected);
        Ok(())
    }

    fn choose_groups(&self, groups: &PackageGroups) -> Result<Vec<String>> {
        for (index, (name, _)) in groups.iter().enumerate() {
            println!("  {:>2}. {name}", index + 1);
        }
        loop {
            let answer = self
                .runner
                .input("Groups (numbers or names, space-separated, Enter for all): ")?;
            if answer.is_empty() {
                return Ok(groups.iter().map(|(name, _)| name.clone()).collect());
            }
            match select_groups(&answer, groups) {
                Ok(selected) => return Ok(selected),
                Err(cause) => eprintln!("{cause}"),
            }
        }
    }
    pub fn install_packages(&mut self) -> Result<()> {
        if self.optional_groups.is_none() {
            self.select_optional_groups()?;
        }
        let mut packages = package_lines(&fs::read_to_string(self.source.join("pkglist.txt"))?)?;
        let groups = self.optional_package_groups()?;
        for selected in self.optional_groups.as_ref().unwrap() {
            let (_, requirements) = groups
                .iter()
                .find(|(name, _)| name == selected)
                .ok_or_else(|| error(format!("Unknown optional group: {selected}")))?;
            for package in requirements {
                if !packages.contains(package) {
                    packages.push(package.clone());
                }
            }
        }
        if packages.is_empty() {
            return Err(error("Package list is empty"));
        }
        let listed = self.output("pacman", &["-Slq"])?;
        let names: HashSet<_> = listed.stdout.lines().collect();
        let mut official = Vec::new();
        let mut foreign = Vec::new();
        for package in packages.iter().map(String::as_str) {
            let in_repo = if listed.success() {
                names.contains(package)
            } else {
                // Preserve individual lookups when a configured repository cannot be listed.
                self.output("pacman", &["-Si", package])?.success()
            };
            if in_repo {
                official.push(package);
            } else {
                foreign.push(package);
            }
        }
        let mut aur = Vec::new();
        if !foreign.is_empty() {
            let mut args = vec!["-T"];
            args.extend_from_slice(&foreign);
            let missing = self.output("pacman", &args)?;
            // Pacman returns 127 and prints the missing dependency names for -T.
            if !missing.success() && missing.code != 127 {
                return Err(error(format!(
                    "Could not check installed package providers: {}",
                    missing.stderr.trim()
                )));
            }
            let missing: HashSet<_> = missing.stdout.lines().collect();
            for package in foreign {
                if missing.contains(package) {
                    aur.push(format!("aur/{package}"));
                } else {
                    self.note(format_args!(
                        "Retaining installed package/provider: {package}"
                    ));
                }
            }
        }
        let mut log = tempfile::Builder::new()
            .prefix("hype-niri-packages-")
            .suffix(".log")
            .tempfile()?;
        if !aur.is_empty() {
            if !self.runner.available("yay") {
                return Err(error(
                    "yay is required for missing AUR packages; install yay, then rerun",
                ));
            }
            let mut args = vec!["-Si", "--aur"];
            args.extend(aur.iter().map(String::as_str));
            let result = self.output("yay", &args)?;
            writeln!(log, "{}{}", result.stdout, result.stderr)?;
            if !result.success() {
                return Err(
                    self.package_failure(log, "AUR metadata lookup failed; no packages installed")
                );
            }
        }
        if !official.is_empty() {
            let mut args = vec!["pacman", "-S", "--needed", "--noconfirm"];
            args.extend(official);
            if let Err(failure) = self.runner.run_logged("sudo", &args, log.path()) {
                return Err(self.package_failure(
                    log,
                    &format!(
                        "Repository installation failed: {failure}. Check mirrors and rerun a full system upgrade"
                    ),
                ));
            }
        }
        if !aur.is_empty() {
            let mut args = vec!["-S", "--aur", "--needed", "--noconfirm=false", "--confirm"];
            args.extend(aur.iter().map(String::as_str));
            if let Err(failure) = self.runner.run_logged("yay", &args, log.path()) {
                return Err(self.package_failure(
                    log,
                    &format!(
                        "AUR installation failed: {failure}. Check provider choices and conflicts"
                    ),
                ));
            }
        }
        let mut args = vec!["-T"];
        args.extend(packages.iter().map(String::as_str));
        let result = self.output("pacman", &args)?;
        writeln!(log, "{}{}", result.stdout, result.stderr)?;
        if !result.success() {
            return Err(self.package_failure(log, "Required packages remain unsatisfied"));
        }
        Ok(())
    }

    fn package_failure(
        &self,
        log: tempfile::NamedTempFile,
        message: &str,
    ) -> Box<dyn std::error::Error> {
        match log.keep() {
            Ok((_, path)) => {
                if let Ok(content) = fs::read_to_string(&path) {
                    let lines = content.lines().collect::<Vec<_>>();
                    for line in &lines[lines.len().saturating_sub(40)..] {
                        self.warn(format_args!("{line}"));
                    }
                }
                error(format!("{message}. Full log: {}", path.display()))
            }
            Err(failure) => error(format!("{message}. Could not preserve log: {failure}")),
        }
    }
}

fn package_lines(contents: &str) -> Result<Vec<String>> {
    let mut packages = Vec::new();
    for line in contents
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
    {
        if !line
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"@._+-".contains(&byte))
            || line.starts_with('-')
        {
            return Err(error(format!("Invalid package requirement: {line}")));
        }
        if !packages.iter().any(|package| package == line) {
            packages.push(line.to_owned());
        }
    }
    Ok(packages)
}

type PackageGroups = Vec<(String, Vec<String>)>;
fn optional_groups(contents: &str) -> Result<PackageGroups> {
    let mut groups: PackageGroups = Vec::new();
    for line in contents
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
    {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
                || groups.iter().any(|(existing, _)| existing == name)
            {
                return Err(error(format!(
                    "Invalid or duplicate optional group: {name}"
                )));
            }
            groups.push((name.into(), Vec::new()));
        } else {
            let (_, packages) = groups
                .last_mut()
                .ok_or_else(|| error("Optional package requires a group heading"))?;
            packages.extend(package_lines(line)?);
        }
    }
    if groups.is_empty() {
        return Err(error("Optional package manifest has no groups"));
    }
    Ok(groups)
}
fn select_groups(answer: &str, groups: &PackageGroups) -> Result<Vec<String>> {
    let mut selected = Vec::new();
    for choice in answer.split_whitespace() {
        let group = match choice.parse::<usize>() {
            Ok(number) => number.checked_sub(1).and_then(|index| groups.get(index)),
            Err(_) => groups.iter().find(|(name, _)| name == choice),
        };
        let (name, _) = group.ok_or_else(|| error(format!("Unknown optional group: {choice}")))?;
        if !selected.contains(name) {
            selected.push(name.clone());
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, Runner};
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    struct Fake {
        calls: Rc<RefCell<Vec<String>>>,
        metadata_fails: bool,
        install_fails: bool,
        verification_fails: bool,
        yay_available: bool,
        answers: RefCell<VecDeque<String>>,
    }
    impl Runner for Fake {
        fn output(&self, p: &str, a: &[&str]) -> Result<CommandOutput> {
            self.calls.borrow_mut().push(format!("{p} {}", a.join(" ")));
            if p == "pacman" && a == ["-Slq"] {
                return Ok(CommandOutput {
                    stdout: "official\n".into(),
                    ..Default::default()
                });
            }
            if p == "pacman" && a.first() == Some(&"-T") {
                let is_validation = a.contains(&"official");
                let unsatisfied =
                    a.contains(&"missing") && (!is_validation || self.verification_fails);
                return Ok(CommandOutput {
                    code: if unsatisfied { 127 } else { 0 },
                    stdout: if unsatisfied {
                        "missing\n".into()
                    } else {
                        String::new()
                    },
                    ..Default::default()
                });
            }
            let failed = p == "yay" && self.metadata_fails;
            Ok(CommandOutput {
                code: i32::from(failed),
                ..Default::default()
            })
        }
        fn run(&self, p: &str, a: &[&str]) -> Result<()> {
            self.calls.borrow_mut().push(format!("{p} {}", a.join(" ")));
            if self.install_fails && p == "yay" {
                Err(crate::error("AUR fixture failed"))
            } else {
                Ok(())
            }
        }
        fn confirm(&self, prompt: &str) -> Result<bool> {
            self.calls.borrow_mut().push(prompt.to_owned());
            Ok(false)
        }
        fn input(&self, prompt: &str) -> Result<String> {
            self.calls.borrow_mut().push(prompt.to_owned());
            self.answers
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| error("Unexpected prompt"))
        }
        fn available(&self, command: &str) -> bool {
            command != "yay" || self.yay_available
        }
    }
    fn fixture(
        metadata_fails: bool,
        install_fails: bool,
        verification_fails: bool,
    ) -> (tempfile::TempDir, Installer, Rc<RefCell<Vec<String>>>) {
        let root = tempfile::tempdir().unwrap();
        let calls = Rc::new(RefCell::new(Vec::new()));
        std::fs::write(
            root.path().join("pkglist.txt"),
            " # ignore\nofficial # desktop\nforeign\nprovider\nmissing\nofficial\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("optional-packages.txt"),
            "[docker]\ndocker\n",
        )
        .unwrap();
        let installer = Installer {
            source: root.path().to_owned(),
            home: root.path().join("home"),
            root: root.path().join("system"),
            backup: None,
            build_dir: None,
            optional_groups: Some(Vec::new()),
            runner: Box::new(Fake {
                calls: calls.clone(),
                metadata_fails,
                install_fails,
                verification_fails,
                yay_available: true,
                answers: RefCell::default(),
            }),
        };
        (root, installer, calls)
    }
    #[test]
    fn custom_selection_deduplicates_and_rejects_unknown_groups() {
        let groups = optional_groups("[docker]\ndocker\n[bluetooth]\nbluez\n").unwrap();
        assert!(select_groups("", &groups).unwrap().is_empty());
        assert_eq!(
            select_groups("1 bluetooth docker 2", &groups).unwrap(),
            ["docker", "bluetooth"]
        );
        for invalid in ["typo", "0", "3"] {
            assert!(select_groups(invalid, &groups).is_err());
        }
        for invalid in ["docker", "[a]\na\n[a]\nb", "[bad group]\na", "[a]\n--bad"] {
            assert!(optional_groups(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn core_install_does_not_require_yay_or_install_unselected_groups() {
        let (root, mut installer, calls) = fixture(false, false, false);
        fs::write(root.path().join("pkglist.txt"), "official\nprovider\n").unwrap();
        installer.optional_groups = None;
        installer.runner = Box::new(Fake {
            calls: calls.clone(),
            metadata_fails: false,
            install_fails: false,
            verification_fails: false,
            yay_available: false,
            answers: RefCell::new(["n".into()].into()),
        });
        installer.install_packages().unwrap();
        assert_eq!(installer.optional_groups, Some(Vec::new()));
        assert_eq!(calls.borrow()[0], "Install extras? [Y/n/custom] ");
        assert!(
            !calls
                .borrow()
                .iter()
                .any(|call| call.starts_with("Groups (")
                    || call.starts_with("yay ")
                    || call.contains("docker"))
        );
    }
    #[test]
    fn accepted_extras_install_and_save_the_selected_groups() {
        let (root, mut installer, calls) = fixture(false, false, false);
        fs::write(root.path().join("pkglist.txt"), "official\n").unwrap();
        fs::write(
            root.path().join("optional-packages.txt"),
            "[hype-flow]\nmissing\n[bluetooth]\nbluez\n",
        )
        .unwrap();
        installer.optional_groups = None;
        installer.runner = Box::new(Fake {
            calls: calls.clone(),
            metadata_fails: false,
            install_fails: false,
            verification_fails: false,
            yay_available: true,
            answers: RefCell::new(["custom".into(), "1".into()].into()),
        });
        installer.install_packages().unwrap();
        installer.save_optional_groups().unwrap();
        assert_eq!(
            fs::read_to_string(installer.user(".config/hype-niri/optional-groups")).unwrap(),
            "hype-flow\n"
        );
        let calls = calls.borrow();
        assert_eq!(calls[0], "Install extras? [Y/n/custom] ");
        assert!(calls[1].starts_with("Groups ("));
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with("yay -S ") && call.ends_with("aur/missing"))
        );
        assert!(!calls.iter().any(|call| call.contains("bluez")));
    }

    #[test]
    fn enter_installs_and_saves_all_extras_including_hype_flow() {
        let (root, mut installer, calls) = fixture(false, false, false);
        fs::write(root.path().join("pkglist.txt"), "official\n").unwrap();
        fs::write(
            root.path().join("optional-packages.txt"),
            "[browser-firefox]\nfirefox\n[browser-brave]\nbrave-bin\n[hype-flow]\nmissing\n",
        )
        .unwrap();
        installer.optional_groups = None;
        installer.runner = Box::new(Fake {
            calls: calls.clone(),
            metadata_fails: false,
            install_fails: false,
            verification_fails: false,
            yay_available: true,
            answers: RefCell::new([String::new()].into()),
        });
        installer.install_packages().unwrap();
        installer.save_optional_groups().unwrap();
        assert_eq!(
            fs::read_to_string(installer.user(".config/hype-niri/optional-groups")).unwrap(),
            "browser-firefox\nbrowser-brave\nhype-flow\n"
        );
        assert!(
            calls
                .borrow()
                .iter()
                .any(|call| call.starts_with("yay -S ") && call.ends_with("aur/missing"))
        );
    }
    #[test]
    fn selected_aur_group_requires_yay_before_repository_installation() {
        let (root, mut installer, calls) = fixture(false, false, false);
        fs::write(root.path().join("pkglist.txt"), "official\n").unwrap();
        fs::write(
            root.path().join("optional-packages.txt"),
            "[docker]\nmissing\n",
        )
        .unwrap();
        installer.optional_groups = None;
        installer.runner = Box::new(Fake {
            calls: calls.clone(),
            metadata_fails: false,
            install_fails: false,
            verification_fails: false,
            yay_available: false,
            answers: RefCell::new(["y".into()].into()),
        });
        assert!(
            installer
                .install_packages()
                .unwrap_err()
                .to_string()
                .contains("yay is required")
        );
        assert!(!calls.borrow().iter().any(|call| call.starts_with("sudo ")));
    }
    #[test]
    fn repository_core_excludes_optional_apps_and_replaced_helpers() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let core = fs::read_to_string(root.join("pkglist.txt")).unwrap();
        let packages: Vec<_> = core
            .lines()
            .map(|line| line.split('#').next().unwrap().trim())
            .collect();
        for optional in [
            "docker",
            "bluez",
            "brave-bin",
            "firefox",
            "neovim",
            "vulkan-intel",
            "wofi",
            "wl-clipboard",
            "cliphist",
            "bemoji",
            "wtype",
            "playerctl",
            "imagemagick",
            "libnotify",
            "lm_sensors",
        ] {
            assert!(
                !packages.contains(&optional),
                "unexpected default: {optional}"
            );
        }
        assert!(root.join("optional-packages.txt").is_file());
    }
    #[test]
    fn retains_foreign_providers_but_upgrades_official_packages() {
        let (_root, mut installer, calls) = fixture(false, false, false);
        installer.install_packages().unwrap();
        let calls = calls.borrow();
        assert_eq!(
            &calls[..2],
            ["pacman -Slq", "pacman -T foreign provider missing"]
        );
        assert!(
            calls
                .iter()
                .any(|c| c == "sudo pacman -S --needed --noconfirm official")
        );
        assert!(calls.iter().any(|c| c == "yay -Si --aur aur/missing"));
        assert!(
            calls
                .iter()
                .any(|c| c == "yay -S --aur --needed --noconfirm=false --confirm aur/missing")
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.contains("aur/foreign") || c.contains("aur/provider"))
        );
        assert_eq!(
            calls.last().unwrap(),
            "pacman -T official foreign provider missing"
        );
    }
    #[test]
    fn failed_aur_preflight_stops_before_any_installation() {
        let (_root, mut installer, calls) = fixture(true, false, false);
        assert!(installer.install_packages().is_err());
        assert!(
            !calls
                .borrow()
                .iter()
                .any(|c| c.starts_with("sudo ") || c.starts_with("yay -S "))
        );
    }
    #[test]
    fn failed_aur_install_does_not_claim_success() {
        let (_root, mut installer, _) = fixture(false, true, false);
        assert!(installer.install_packages().is_err());
    }
    #[test]
    fn unsatisfied_packages_stop_even_after_successful_installs() {
        let (_root, mut installer, _) = fixture(false, false, true);
        assert!(installer.install_packages().is_err());
    }
}
