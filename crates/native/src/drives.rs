use crate::{Result, bus, dialog, err, notify, runtime};
use gtk::{gio, glib, prelude::*};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, fs::symlink},
    path::{Path, PathBuf},
    time::Duration,
};
const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
type Properties = HashMap<String, glib::Variant>;
type Interfaces = HashMap<String, Properties>;
type Objects = HashMap<glib::variant::ObjectPath, Interfaces>;
async fn objects(conn: &gio::DBusConnection) -> Result<Objects> {
    bus::call(
        conn,
        SERVICE,
        ROOT,
        "org.freedesktop.DBus.ObjectManager",
        "GetManagedObjects",
        None,
        5000,
    )
    .await?
    .get::<(Objects,)>()
    .map(|v| v.0)
    .ok_or_else(|| err("Invalid UDisks device list"))
}
fn text<'a>(properties: &'a Properties, key: &str) -> Option<&'a str> {
    properties.get(key)?.str()
}
fn bytes_path(value: &glib::Variant) -> Option<PathBuf> {
    let mut bytes = value.get::<Vec<u8>>()?;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}
fn mounts(interfaces: &Interfaces) -> Vec<PathBuf> {
    interfaces
        .get(FILESYSTEM)
        .and_then(|p| p.get("MountPoints"))
        .and_then(|v| v.get::<Vec<Vec<u8>>>())
        .unwrap_or_default()
        .into_iter()
        .map(|mut bytes| {
            while bytes.last() == Some(&0) {
                bytes.pop();
            }
            PathBuf::from(std::ffi::OsString::from_vec(bytes))
        })
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}
fn supported(fstype: &str) -> bool {
    matches!(
        fstype,
        "ntfs" | "exfat" | "btrfs" | "ext2" | "ext3" | "ext4" | "xfs"
    )
}
fn sanitize_name(name: &str) -> String {
    let value = name
        .replace(['/', '\\'], "-")
        .replace(['\n', '\r', '\t'], " ")
        .trim()
        .to_owned();
    if value.is_empty() || value == "." || value == ".." {
        "Drive".into()
    } else {
        value
    }
}
fn mountinfo_paths(contents: &[u8]) -> Vec<PathBuf> {
    let mut mounts: Vec<_> = contents
        .split(|&byte| byte == b'\n')
        .filter_map(|line| line.split(|&byte| byte == b' ').nth(4))
        .map(|mut field| {
            let mut path = Vec::with_capacity(field.len());
            while let Some((&byte, rest)) = field.split_first() {
                // mountinfo escapes space, tab, newline, and backslash as octal.
                let escaped = match field.get(..4) {
                    Some(b"\\040") => Some(b' '),
                    Some(b"\\011") => Some(b'\t'),
                    Some(b"\\012") => Some(b'\n'),
                    Some(b"\\134") => Some(b'\\'),
                    _ => None,
                };
                if let Some(decoded) = escaped {
                    path.push(decoded);
                    field = &field[4..];
                } else {
                    path.push(byte);
                    field = rest;
                }
            }
            PathBuf::from(OsString::from_vec(path))
        })
        .collect();
    mounts.sort_unstable();
    mounts.dedup();
    mounts
}
fn links(root: &Path, media: &Path, devices: &Objects, kernel_mounts: &[PathBuf]) -> Result<()> {
    fs::create_dir_all(root)?;
    for entry in fs::read_dir(root)?.flatten() {
        if entry.file_type()?.is_symlink() && !entry.path().exists() {
            fs::remove_file(entry.path())?;
        }
    }
    let mut entries: Vec<_> = devices.iter().collect();
    entries.sort_by_key(|(path, _)| path.to_string());
    let mut candidates = Vec::new();
    for (_, interfaces) in entries {
        let Some(block) = interfaces.get(BLOCK) else {
            continue;
        };
        let label = text(block, "IdLabel")
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                interfaces
                    .get("org.freedesktop.UDisks2.Partition")
                    .and_then(|p| text(p, "Name"))
                    .filter(|v| !v.trim().is_empty())
            });
        candidates.extend(mounts(interfaces).into_iter().map(|mount| (mount, label)));
    }
    candidates.extend(kernel_mounts.iter().cloned().map(|mount| (mount, None)));

    let mut linked = HashSet::new();
    let mut used = HashSet::new();
    for (mount, label) in &candidates {
        if !mount.starts_with(media) || mount == media || !linked.insert(mount) {
            continue;
        }
        let basename = mount.file_name().unwrap_or_default().to_string_lossy();
        let base = sanitize_name(label.unwrap_or(&basename));
        let mut name = base.clone();
        let mut suffix = 2;
        while used.contains(&name)
            || fs::symlink_metadata(root.join(&name)).is_ok_and(|m| !m.file_type().is_symlink())
        {
            name = format!("{base}-{suffix}");
            suffix += 1;
        }
        used.insert(name.clone());
        let destination = root.join(name);
        let temporary = tempfile::Builder::new()
            .prefix(".drive-link-")
            .tempfile_in(root)?;
        let path = temporary.into_temp_path();
        fs::remove_file(&path)?;
        symlink(mount, &path)?;
        fs::rename(&path, destination)?;
    }
    Ok(())
}
async fn mount(conn: &gio::DBusConnection, path: &str, fstype: &str) -> Result<()> {
    let options = Properties::new();
    let first = bus::call(
        conn,
        SERVICE,
        path,
        FILESYSTEM,
        "Mount",
        Some(&(options,).to_variant()),
        15000,
    )
    .await;
    if first.is_ok() {
        return Ok(());
    }
    if fstype == "ntfs" {
        let options = Properties::from([("options".into(), "ro".to_variant())]);
        if bus::call(
            conn,
            SERVICE,
            path,
            FILESYSTEM,
            "Mount",
            Some(&(options,).to_variant()),
            15000,
        )
        .await
        .is_ok()
        {
            let _=notify::send(0,"Mounted Windows volume read-only","Windows left this NTFS volume dirty or hibernated. Run chkdsk and fully shut down Windows for write access.",None,"drives").await;
            return Ok(());
        }
    }
    Err(first.unwrap_err())
}
async fn unlock(conn: &gio::DBusConnection, devices: &Objects) -> Result<()> {
    for (path, interfaces) in devices {
        let Some(encrypted) = interfaces.get(ENCRYPTED) else {
            continue;
        };
        if encrypted
            .get("CleartextDevice")
            .and_then(|v| v.get::<glib::variant::ObjectPath>())
            .is_some_and(|p| p.as_str() != "/")
        {
            continue;
        }
        let Some(block) = interfaces.get(BLOCK) else {
            continue;
        };
        if text(block, "IdType") != Some("crypto_LUKS") {
            continue;
        }
        let name = interfaces
            .get("org.freedesktop.UDisks2.Partition")
            .and_then(|p| text(p, "Name"))
            .filter(|v| !v.trim().is_empty())
            .or_else(|| text(block, "IdLabel").filter(|v| !v.trim().is_empty()))
            .map(String::from)
            .unwrap_or_else(|| {
                block
                    .get("PreferredDevice")
                    .and_then(bytes_path)
                    .unwrap_or_default()
                    .display()
                    .to_string()
            });
        let Some(passphrase) = dialog::prompt(&format!("Unlock {name}"), true).await? else {
            continue;
        };
        if passphrase.is_empty() {
            continue;
        }
        let options = Properties::new();
        if bus::call(
            conn,
            SERVICE,
            path.as_str(),
            ENCRYPTED,
            "Unlock",
            Some(&(passphrase, options).to_variant()),
            15000,
        )
        .await
        .is_err()
        {
            let _ = notify::send(
                0,
                "Unlock failed",
                &format!("Wrong passphrase for {name}, or device busy."),
                None,
                "drives",
            )
            .await;
        }
    }
    Ok(())
}
pub async fn run(_command: &str, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err(err("Usage: hype-control open-drives"));
    }
    let Some(_lock) = runtime::lock("hype-open-drives.lock", Duration::ZERO)? else {
        return Ok(());
    };
    let root = runtime::home()?.join("drives");
    fs::create_dir_all(&root)?;
    let mut snapshot = Objects::new();
    let mounting: Result<()> = async {
        let manager = bus::system().await;
        if let Ok(conn) = manager {
            match objects(&conn).await {
                Ok(initial) => {
                    snapshot = initial;
                    unlock(&conn, &snapshot).await?;
                    let mut tried = HashSet::new();
                    glib::future_with_timeout(Duration::from_secs(20), async {
                        loop {
                            snapshot = objects(&conn).await?;
                            let mut pending = false;
                            for (path, interfaces) in &snapshot {
                                let Some(block) = interfaces.get(BLOCK) else {
                                    continue;
                                };
                                let fstype = text(block, "IdType").unwrap_or_default();
                                let ignored = block
                                    .get("HintIgnore")
                                    .and_then(|v| v.get::<bool>())
                                    .unwrap_or(false);
                                if supported(fstype)
                                    && !ignored
                                    && interfaces.contains_key(FILESYSTEM)
                                    && mounts(interfaces).is_empty()
                                    && tried.insert(path.clone())
                                {
                                    pending = true;
                                    if let Err(error) = mount(&conn, path.as_str(), fstype).await {
                                        let _ = notify::send(
                                            0,
                                            "Could not mount drive",
                                            &error.to_string(),
                                            None,
                                            "drives",
                                        )
                                        .await;
                                    }
                                }
                            }
                            if !pending {
                                break;
                            }
                            glib::timeout_future(Duration::from_secs(1)).await;
                        }
                        Ok::<_, Box<dyn std::error::Error>>(())
                    })
                    .await
                    .map_err(|_| err("Drive mounting reached its 20-second deadline"))??;
                }
                Err(error) => {
                    let _ = notify::send(
                        0,
                        "Drive mounting unavailable",
                        &error.to_string(),
                        None,
                        "drives",
                    )
                    .await;
                }
            }
        } else {
            let _ = notify::send(
                0,
                "Drive mounting unavailable",
                "UDisks2 is unavailable.",
                None,
                "drives",
            )
            .await;
        }
        Ok(())
    }
    .await;
    let media = PathBuf::from("/run/media")
        .join(std::env::var_os("USER").unwrap_or_else(|| runtime::uid().to_string().into()));
    let kernel_mounts = fs::read("/proc/self/mountinfo").map(|contents| mountinfo_paths(&contents));
    let aliases = links(
        &root,
        &media,
        &snapshot,
        kernel_mounts.as_deref().unwrap_or_default(),
    );
    let mounting = mounting
        .and(aliases)
        .and(kernel_mounts.map(drop).map_err(Into::into));
    drop(_lock);
    let uri = gio::File::for_path(root).uri();
    let launched =
        gio::AppInfo::launch_default_for_uri_future(&uri, None::<&gio::AppLaunchContext>).await;
    if let Err(error) = mounting {
        let _ = notify::send(
            0,
            "Drive mounting incomplete",
            &error.to_string(),
            None,
            "drives",
        )
        .await;
    }
    launched?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_cannot_escape_drive_directory() {
        assert_eq!(sanitize_name(" ../a\\b\n "), "..-a-b");
        assert_eq!(sanitize_name(".."), "Drive");
        assert_eq!(sanitize_name("\n\t"), "Drive");
    }
    #[test]
    fn mount_links_keep_real_files_and_deduplicate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("drives");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("Work"), "keep").unwrap();
        let media = temp.path().join("media");
        let a = media.join("a");
        let b = media.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let mut devices = Objects::new();
        for (index, mount) in [&a, &b].iter().enumerate() {
            use std::os::unix::ffi::OsStrExt;
            let mut bytes = mount.as_os_str().as_bytes().to_vec();
            bytes.push(0);
            devices.insert(
                glib::variant::ObjectPath::try_from(format!("/device{index}")).unwrap(),
                Interfaces::from([
                    (
                        BLOCK.into(),
                        Properties::from([("IdLabel".into(), "Work".to_variant())]),
                    ),
                    (
                        FILESYSTEM.into(),
                        Properties::from([("MountPoints".into(), vec![bytes].to_variant())]),
                    ),
                ]),
            );
        }
        let c = media.join("manual").join("Work");
        fs::create_dir_all(&c).unwrap();
        let sibling = media.with_file_name("media-other").join("Outside");
        links(
            &root,
            &media,
            &devices,
            &[a.clone(), c.clone(), media.clone(), sibling],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("Work")).unwrap(), "keep");
        assert_eq!(fs::read_link(root.join("Work-2")).unwrap(), a);
        assert_eq!(fs::read_link(root.join("Work-3")).unwrap(), b);
        assert_eq!(fs::read_link(root.join("Work-4")).unwrap(), c);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 4);
    }
    #[test]
    fn mountinfo_decodes_paths_and_deduplicates_mounts() {
        let contents = b"20 19 8:1 / /run/media/me rw - ext4 /dev/sda1 rw\n\
            21 20 8:2 / /run/media/me/My\\040Disk\\011\\012\\134040\xff rw - ext4 /dev/sda2 rw\n\
            22 21 8:2 / /run/media/me/My\\040Disk\\011\\012\\134040\xff rw - ext4 /dev/sda2 rw\n";
        assert_eq!(
            mountinfo_paths(contents),
            [
                PathBuf::from("/run/media/me"),
                PathBuf::from(OsString::from_vec(
                    b"/run/media/me/My Disk\t\n\\040\xff".to_vec()
                )),
            ]
        );
    }
}
