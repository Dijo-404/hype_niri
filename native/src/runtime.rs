use crate::{Result, err};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
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
pub fn lock(name: &str, timeout: Duration) -> Result<Option<Lock>> {
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
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
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
pub fn signal_waybar(offset: i32) -> Result<()> {
    let signal = libc::SIGRTMIN() + offset;
    for entry in fs::read_dir("/proc")?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if entry.metadata().is_ok_and(|m| m.uid() == uid())
            && fs::read_to_string(entry.path().join("comm")).is_ok_and(|s| s.trim() == "waybar")
        {
            unsafe {
                libc::kill(pid, signal);
            }
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
