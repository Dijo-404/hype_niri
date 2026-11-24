use crate::{Installer, Result, config::path_arg, error};
use std::{fs, os::unix::fs::symlink, path::Path};

const REPOSITORY: &str = "https://github.com/Dijo-404/hype_flow.git";
const REVISION: &str = "7938c5a8e883f18499aded4b0884d8c2ca668d0f";
// Remove the patch when the pinned Hype Flow revision includes headless setup and lazy startup.
const INTEGRATION: &str = include_str!("hype-flow.patch");

impl Installer {
    pub(crate) fn setup_hype_flow(&self) -> Result<()> {
        if !self.selected("hype-flow") {
            return Ok(());
        }
        self.install_hype_flow()
            .map_err(|cause| error(format!("Hype Flow installation failed: {cause}")))
    }

    fn install_hype_flow(&self) -> Result<()> {
        println!("Building Hype Flow and setting up offline dictation");
        let staging = tempfile::Builder::new().prefix("hype-flow-").tempdir()?;
        let checkout = staging.path().join("source");
        let target = staging.path().join("target");
        let patch = staging.path().join("hype-flow.patch");
        let host = self.cargo_host()?;
        let repo = path_arg(&checkout)?;
        fs::write(&patch, INTEGRATION)?;
        self.command("git", &["init", "--quiet", repo])?;
        self.command(
            "git",
            &[
                "-C",
                repo,
                "fetch",
                "--quiet",
                "--depth=1",
                "--",
                REPOSITORY,
                REVISION,
            ],
        )?;
        self.command("git", &["-C", repo, "checkout", "--detach", REVISION])?;
        self.command("git", &["-C", repo, "apply", "--", path_arg(&patch)?])?;
        self.command(
            "cargo",
            &[
                "build",
                "--release",
                "--locked",
                "--jobs=2",
                "--workspace",
                "--target",
                &host,
                "--manifest-path",
                path_arg(&checkout.join("Cargo.toml"))?,
                "--target-dir",
                path_arg(&target)?,
            ],
        )?;
        // Hype Flow's install script reads the host release from source/target/release.
        symlink(target.join(host), checkout.join("target"))?;
        let prefix = format!("HYPE_FLOW_PREFIX={}", path_arg(&self.user(".local"))?);
        self.command(
            "env",
            &[&prefix, "sh", path_arg(&checkout.join("tools/install.sh"))?],
        )?;
        let flow = self.user(".local/bin/hype-flow");
        let niri_config = format!(
            "NIRI_CONFIG={}",
            path_arg(&self.user(".config/niri/config.kdl"))?
        );
        self.command("env", &[&niri_config, path_arg(&flow)?, "--setup"])?;

        if std::env::var_os("NIRI_SOCKET").is_some_and(|socket| Path::new(&socket).exists())
            && let Err(cause) = self.command(
                "niri",
                &[
                    "msg",
                    "action",
                    "spawn",
                    "--",
                    path_arg(&flow)?,
                    "--background",
                ],
            )
        {
            eprintln!(
                "Hype Flow will start at the next Niri login. Launch request failed: {cause}"
            );
        }
        Ok(())
    }
}
