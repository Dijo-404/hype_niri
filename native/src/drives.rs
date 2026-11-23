use crate::{Result, bus, dialog, err, notify, runtime};
use gtk::{gio, glib, prelude::*};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::{ffi::OsStringExt, fs::symlink},
    path::{Path, PathBuf},
    time::{Duration, Instant},
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
fn links(root: &Path, media: &Path, devices: &Objects) -> Result<()> {
    fs::create_dir_all(root)?;
    for entry in fs::read_dir(root)?.flatten() {
        if entry.file_type()?.is_symlink() && !entry.path().exists() {
            fs::remove_file(entry.path())?;
        }
    }
    let mut used = HashSet::new();
    let mut entries: Vec<_> = devices.iter().collect();
    entries.sort_by_key(|(path, _)| path.to_string());
    for (_, interfaces) in entries {
        let Some(block) = interfaces.get(BLOCK) else {
            continue;
        };
        for mount in mounts(interfaces) {
            if !mount.starts_with(media) || mount == media {
                continue;
            }
            let label = text(block, "IdLabel")
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    interfaces
                        .get("org.freedesktop.UDisks2.Partition")
                        .and_then(|p| text(p, "Name"))
                        .filter(|v| !v.trim().is_empty())
                })
                .map(String::from)
                .unwrap_or_else(|| {
                    mount
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
            let base = sanitize_name(&label);
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
            symlink(&mount, &path)?;
            fs::rename(&path, destination)?;
        }
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
        let Some(passphrase) = dialog::prompt(&format!("Unlock {name}"), true)? else {
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
    let mounting: Result<()> = async {
        let manager = bus::system().await;
        if let Ok(conn) = manager {
            match objects(&conn).await {
                Ok(initial) => {
                    unlock(&conn, &initial).await?;
                    let deadline = Instant::now() + Duration::from_secs(20);
                    loop {
                        let snapshot = objects(&conn).await?;
                        let mut pending = false;
                        for (path, interfaces) in &snapshot {
                            let Some(block) = interfaces.get(BLOCK) else {
                                continue;
                            };
                            let fstype = text(block, "IdType").unwrap_or_default();
                            if supported(fstype)
                                && interfaces.contains_key(FILESYSTEM)
                                && mounts(interfaces).is_empty()
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
                        let snapshot = objects(&conn).await?;
                        let media = PathBuf::from("/run/media").join(
                            std::env::var("USER").unwrap_or_else(|_| runtime::uid().to_string()),
                        );
                        links(&root, &media, &snapshot)?;
                        if !pending || Instant::now() >= deadline {
                            break;
                        }
                        glib::timeout_future(Duration::from_secs(1)).await;
                    }
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
    drop(_lock);
    let uri = gio::File::for_path(root).uri();
    gio::AppInfo::launch_default_for_uri_future(&uri, None::<&gio::AppLaunchContext>).await?;
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
        links(&root, &media, &devices).unwrap();
        assert_eq!(fs::read_to_string(root.join("Work")).unwrap(), "keep");
        assert_eq!(fs::read_link(root.join("Work-2")).unwrap(), a);
        assert_eq!(fs::read_link(root.join("Work-3")).unwrap(), b);
    }
}
