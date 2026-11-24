use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub mod config;
pub mod core;
mod flow;
pub mod packages;
pub mod system;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    io::Error::other(message.into()).into()
}

fn terminal_color(is_terminal: bool) -> bool {
    is_terminal
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TERM").as_deref() != Some("dumb".as_ref())
}
#[derive(Debug, Default, Clone)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
impl CommandOutput {
    pub fn success(&self) -> bool {
        self.code == 0
    }
}
pub trait Runner {
    fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput>;
    fn run(&self, program: &str, args: &[&str]) -> Result<()>;
    fn confirm(&self, prompt: &str) -> Result<bool>;
    fn input(&self, prompt: &str) -> Result<String>;
    fn available(&self, program: &str) -> bool;
    fn run_logged(&self, program: &str, args: &[&str], _log: &Path) -> Result<()> {
        self.run(program, args)
    }
}
pub struct ProcessRunner;
impl Runner for ProcessRunner {
    fn run_logged(&self, program: &str, args: &[&str], log: &Path) -> Result<()> {
        use std::{
            fs::OpenOptions,
            io::Read,
            sync::{Arc, Mutex},
        };
        let file = Arc::new(Mutex::new(
            OpenOptions::new().create(true).append(true).open(log)?,
        ));
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| error("Missing command stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| error("Missing command stderr"))?;
        fn stream(
            mut input: impl Read,
            mut terminal: impl Write,
            log: Arc<Mutex<std::fs::File>>,
        ) -> io::Result<()> {
            let mut buffer = [0u8; 4096];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    return Ok(());
                }
                terminal.write_all(&buffer[..count])?;
                terminal.flush()?;
                log.lock()
                    .map_err(|_| io::Error::other("Log lock failed"))?
                    .write_all(&buffer[..count])?;
            }
        }
        let log_out = file.clone();
        let out = std::thread::spawn(move || stream(stdout, io::stdout(), log_out));
        let err = std::thread::spawn(move || stream(stderr, io::stderr(), file));
        let status = child.wait()?;
        for thread in [out, err] {
            thread
                .join()
                .map_err(|_| error("Command log reader failed"))??;
        }
        if status.success() {
            Ok(())
        } else {
            Err(error(format!("{program} failed ({status})")))
        }
    }
    fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(error(format!("{program} failed ({status})")))
        }
    }
    fn confirm(&self, prompt: &str) -> Result<bool> {
        loop {
            let answer = self.input(&format!("{prompt} [y/n]: "))?;
            match answer.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => eprintln!("Please answer y or n."),
            }
        }
    }
    fn input(&self, prompt: &str) -> Result<String> {
        if terminal_color(io::stderr().is_terminal()) {
            eprint!("  \x1b[1;33m?\x1b[0m {prompt}");
        } else {
            eprint!("{prompt}");
        }
        io::stderr().flush()?;
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer)? == 0 {
            return Err(error("Terminal input ended; installation stopped"));
        }
        Ok(answer.trim().to_owned())
    }
    fn available(&self, program: &str) -> bool {
        available(program)
    }
}
pub fn available(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: PathBuf| {
        p.is_file()
            && p.metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return executable(PathBuf::from(program));
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|p| executable(p.join(program))))
}
pub struct Installer {
    pub source: PathBuf,
    pub home: PathBuf,
    pub root: PathBuf,
    pub backup: Option<PathBuf>,
    pub runner: Box<dyn Runner>,
    /// None means package groups have not been selected yet.
    pub optional_groups: Option<Vec<String>>,
    pub build_dir: Option<tempfile::TempDir>,
}
impl Installer {
    pub fn selected(&self, group: &str) -> bool {
        self.optional_groups
            .as_ref()
            .is_some_and(|groups| groups.iter().any(|name| name == group))
    }
    pub fn user(&self, relative: &str) -> PathBuf {
        self.home.join(relative)
    }
    pub fn system(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }
    pub fn confirm(&self, prompt: &str) -> Result<bool> {
        self.runner.confirm(prompt)
    }
    pub fn command(&self, program: &str, args: &[&str]) -> Result<()> {
        self.runner.run(program, args)
    }
    pub fn output(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        self.runner.output(program, args)
    }
    pub fn write_system(&self, absolute: &str, contents: &[u8], mode: u32) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let destination = self.system(absolute);
        if self.root != Path::new("/") || unsafe { libc::geteuid() } == 0 {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut staged = tempfile::NamedTempFile::new_in(
                destination
                    .parent()
                    .ok_or_else(|| error("No destination parent"))?,
            )?;
            staged.write_all(contents)?;
            staged
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(mode))?;
            staged.persist(&destination).map_err(|e| e.error)?;
            return Ok(());
        }
        let mut staged = tempfile::NamedTempFile::new()?;
        staged.write_all(contents)?;
        self.command(
            "sudo",
            &[
                "install",
                "-Dm",
                &format!("{mode:o}"),
                staged
                    .path()
                    .to_str()
                    .ok_or_else(|| error("Invalid staging path"))?,
                destination
                    .to_str()
                    .ok_or_else(|| error("Invalid destination"))?,
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_command_does_not_look_successful() {
        assert!(
            !CommandOutput {
                code: 1,
                ..Default::default()
            }
            .success()
        );
    }
}
