use crate::{Installer, Result, error};
use std::{fs, io::Write};

impl Installer {
    pub fn install_packages(&mut self) -> Result<()> {
        let contents = fs::read_to_string(self.source.join("pkglist.txt"))?;
        let mut packages = Vec::new();
        for line in contents
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default())
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            if line.split_whitespace().count() != 1 || line.starts_with('-') {
                return Err(error(format!("Invalid package requirement: {line}")));
            }
            if !packages.iter().any(|p| p == line) {
                packages.push(line.to_owned());
            }
        }
        if packages.is_empty() {
            return Err(error("Package list is empty"));
        }
        let mut official = Vec::new();
        let mut aur = Vec::new();
        for package in &packages {
            if self.output("pacman", &["-Si", package])?.success() {
                official.push(package.as_str());
            } else if !self.output("pacman", &["-T", package])?.success() {
                aur.push(format!("aur/{package}"));
            } else {
                println!("Retaining installed package/provider: {package}");
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
            // The production terminal is checked before phases run; command input stays inherited.
            let mut args = vec!["-Si", "--aur"];
            args.extend(aur.iter().map(String::as_str));
            let result = self.output("yay", &args)?;
            writeln!(log, "{}{}", result.stdout, result.stderr)?;
            if !result.success() {
                return Err(package_failure(
                    log,
                    "AUR metadata lookup failed; no packages installed",
                ));
            }
        }
        if !official.is_empty() {
            let mut args = vec!["pacman", "-S", "--needed", "--noconfirm"];
            args.extend(official);
            if let Err(failure) = self.runner.run_logged("sudo", &args, log.path()) {
                return Err(package_failure(
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
                return Err(package_failure(
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
            return Err(package_failure(log, "Required packages remain unsatisfied"));
        }
        Ok(())
    }
}

fn package_failure(log: tempfile::NamedTempFile, message: &str) -> Box<dyn std::error::Error> {
    match log.keep() {
        Ok((_, path)) => {
            if let Ok(content) = fs::read_to_string(&path) {
                let lines = content.lines().collect::<Vec<_>>();
                for line in &lines[lines.len().saturating_sub(40)..] {
                    eprintln!("{line}");
                }
            }
            error(format!("{message}. Full log: {}", path.display()))
        }
        Err(failure) => error(format!("{message}. Could not preserve log: {failure}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, Runner};
    use std::{cell::RefCell, rc::Rc};

    struct Fake {
        calls: Rc<RefCell<Vec<String>>>,
        metadata_fails: bool,
        install_fails: bool,
        verification_fails: bool,
    }
    impl Runner for Fake {
        fn output(&self, p: &str, a: &[&str]) -> Result<CommandOutput> {
            self.calls.borrow_mut().push(format!("{p} {}", a.join(" ")));
            let failed = p == "pacman" && a.first() == Some(&"-Si") && a[1] != "official"
                || p == "pacman" && a.first() == Some(&"-T") && a.len() == 2 && a[1] == "missing"
                || p == "yay" && self.metadata_fails
                || p == "pacman"
                    && a.first() == Some(&"-T")
                    && a.len() > 2
                    && self.verification_fails;
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
        fn confirm(&self, _: &str) -> Result<bool> {
            Ok(false)
        }
        fn input(&self, _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn available(&self, _: &str) -> bool {
            true
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
        let installer = Installer {
            source: root.path().to_owned(),
            home: root.path().join("home"),
            root: root.path().join("system"),
            backup: None,
            build_dir: None,
            runner: Box::new(Fake {
                calls: calls.clone(),
                metadata_fails,
                install_fails,
                verification_fails,
            }),
        };
        (root, installer, calls)
    }
    #[test]
    fn retains_foreign_providers_but_upgrades_official_packages() {
        let (_root, mut installer, calls) = fixture(false, false, false);
        installer.install_packages().unwrap();
        let calls = calls.borrow();
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
