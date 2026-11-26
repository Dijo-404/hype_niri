use crate::{Result, err};
use std::os::unix::process::CommandExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| err("HOME is unset"))
}
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().unwrap_or_default().join(".config"))
}
pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().unwrap_or_default().join(".local/state"))
}
pub fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

pub(crate) struct Process(OwnedFd);

impl Process {
    pub(crate) fn open(pid: u32) -> std::io::Result<Option<Self>> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Ok(None);
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd >= 0 {
            return Ok(Some(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) })));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(error)
        }
    }

    pub(crate) fn terminate(&self) -> std::io::Result<()> {
        let sent = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if sent < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }

    pub(crate) fn exited(&self) -> std::io::Result<bool> {
        let mut poll = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        if ready < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(ready > 0)
    }
}
fn owned_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.uid() == uid())
}
fn private_dir(path: &Path) -> bool {
    owned_dir(path) && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.mode() & 0o077 == 0)
}
pub fn dir() -> Result<PathBuf> {
    let preferred = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", uid())));
    resolve_dir(
        &preferred,
        &PathBuf::from(format!("/tmp/hype-niri-{}", uid())),
    )
}
fn resolve_dir(preferred: &Path, fallback: &Path) -> Result<PathBuf> {
    if preferred.is_absolute()
        && private_dir(preferred)
        && tempfile::NamedTempFile::new_in(preferred).is_ok()
    {
        return Ok(preferred.into());
    }
    fallback_dir(fallback)
}
fn fallback_dir(path: &Path) -> Result<PathBuf> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if !owned_dir(path) {
        return Err(err("Unsafe runtime directory"));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = directory.metadata()?;
    if metadata.uid() != uid() {
        return Err(err("Runtime directory owner changed"));
    }
    directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    Ok(path.into())
}
pub struct Lock(File);
fn lock_file(name: &str) -> Result<File> {
    if Path::new(name).components().count() != 1 {
        return Err(err("Invalid lock name"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir()?.join(name))?;
    if file.metadata()?.uid() != uid() || !file.metadata()?.is_file() {
        return Err(err("Unsafe lock file"));
    }
    Ok(file)
}
pub(crate) fn lock_blocking(name: &str) -> Result<Lock> {
    let file = lock_file(name)?;
    file.lock()?;
    Ok(Lock(file))
}
pub fn lock(name: &str, timeout: Duration) -> Result<Option<Lock>> {
    let file = lock_file(name)?;
    let deadline = Instant::now() + timeout;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(Lock(file)));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
impl Lock {
    pub(crate) fn inherit(&self, command: &mut Command) {
        let fd = self.0.as_raw_fd();
        // flock stays held until the last copy of this file descriptor closes.
        // Change CLOEXEC in the child only, leaving all other locks private.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(err(format!(
            "Refusing symlink destination: {}",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| err("Missing parent directory"))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temp.write_all(data)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub(crate) fn matches_process(pid: i32, name: &str) -> bool {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    let mut named = false;
    let mut live = false;
    let mut owned = false;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("Name:") {
            named = value.trim() == name;
        } else if let Some(value) = line.strip_prefix("State:") {
            live = !value.trim_start().starts_with(['Z', 'X', 'x']);
        } else if let Some(value) = line.strip_prefix("Uid:") {
            owned = value
                .split_whitespace()
                .nth(1)
                .and_then(|uid| uid.parse().ok())
                == Some(uid());
            break;
        }
    }
    named && live && owned
}
pub(crate) fn named_processes(name: &str) -> impl Iterator<Item = i32> {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(move |pid| matches_process(*pid, name))
}
pub fn signal_waybar(offset: i32) -> Result<()> {
    let signal = libc::SIGRTMIN() + offset;
    for pid in named_processes("waybar") {
        unsafe {
            libc::kill(pid, signal);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_rejects_links_and_repairs_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("runtime");
        fallback_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o777)).unwrap();
        fallback_dir(&target).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o700);
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(fallback_dir(&link).is_err());
    }
    #[test]
    fn atomic_write_preserves_link_targets() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("state");
        atomic_write(&target, b"first").unwrap();
        atomic_write(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(atomic_write(&link, b"bad").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"second");
    }
    #[test]
    fn preferred_directory_requires_private_permissions_without_chmod() {
        let temp = tempfile::tempdir().unwrap();
        let preferred = temp.path().join("preferred");
        fs::create_dir(&preferred).unwrap();
        let fallback = temp.path().join("fallback");
        for mode in [0o777, 0o770, 0o755, 0o750] {
            fs::set_permissions(&preferred, fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(resolve_dir(&preferred, &fallback).unwrap(), fallback);
            assert_eq!(fs::metadata(&preferred).unwrap().mode() & 0o777, mode);
        }
        fs::set_permissions(&preferred, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(resolve_dir(&preferred, &fallback).unwrap(), preferred);
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&preferred, &link).unwrap();
        assert_eq!(resolve_dir(&link, &fallback).unwrap(), fallback);
    }
}
