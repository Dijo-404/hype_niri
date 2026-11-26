use crate::{Result, dialog, err, runtime};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    net::Shutdown,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use wl_clipboard_rs::{
    copy, paste,
    watch::{ClipboardEvent, ClipboardType, Watcher},
};

const MAX_ENTRIES: usize = 750;
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILE_BYTES: usize = MAX_TOTAL_BYTES + MAX_ENTRIES * 300 + 16;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_TIMEOUT: Duration = Duration::from_secs(4);
const IMPORT_TIMEOUT: Duration = Duration::from_secs(30);
const IMPORT_LIST_BYTES: usize = 1024 * 1024;
const OWNER_TOKEN_PREFIX: &str = "application/x-hype-clipboard-";
const LEGACY_MAGIC: &[u8] = b"HYPECLIP1";
const MAGIC: &[u8] = b"HYPECLIP2";
const BLOB_BYTES: usize = 64 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Entry {
    mime: String,
    data: Vec<u8>,
}
impl Entry {
    fn validate(&self) -> Result<()> {
        if !supported_mime(&self.mime) || self.data.is_empty() || self.data.len() > MAX_ENTRY_BYTES
        {
            return Err(err("Invalid clipboard entry or size limit exceeded"));
        }
        Ok(())
    }
    fn write_to(&self, mut writer: impl Write, blobs: bool) -> Result<()> {
        self.validate()?;
        writer.write_all(&(self.mime.len() as u16).to_le_bytes())?;
        writer.write_all(&(self.data.len() as u32).to_le_bytes())?;
        writer.write_all(self.mime.as_bytes())?;
        if blobs && self.data.len() > BLOB_BYTES {
            writer.write_all(blob_name(&self.data).as_bytes())?;
        } else {
            writer.write_all(&self.data)?;
        }
        Ok(())
    }
    fn read_from(
        mut reader: impl Read,
        budget: usize,
        blobs: Option<&Path>,
    ) -> Result<Option<Self>> {
        let mut mime_length = [0; 2];
        let mut data_length = [0; 4];
        reader.read_exact(&mut mime_length)?;
        reader.read_exact(&mut data_length)?;
        let mime_length = u16::from_le_bytes(mime_length) as usize;
        let data_length = u32::from_le_bytes(data_length) as usize;
        if mime_length > 255 || data_length > MAX_ENTRY_BYTES || data_length > budget {
            return Err(err("Clipboard history size limit exceeded"));
        }
        let mut mime = vec![0; mime_length];
        reader.read_exact(&mut mime)?;
        let mime = String::from_utf8(mime)?;
        if !supported_mime(&mime) || data_length == 0 {
            return Err(err("Invalid clipboard entry"));
        }
        let data = match blobs {
            Some(blobs) if data_length > BLOB_BYTES => {
                let mut name = [0; 64];
                reader.read_exact(&mut name)?;
                let Ok(data) = read_blob(blobs, std::str::from_utf8(&name)?, data_length) else {
                    return Ok(None);
                };
                data
            }
            _ => {
                let mut data = vec![0; data_length];
                reader.read_exact(&mut data)?;
                data
            }
        };
        Ok(Some(Self { mime, data }))
    }
    fn id(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.mime.as_bytes());
        hash.update([0]);
        hash.update(&self.data);
        format!("{:x}", hash.finalize())
    }
    fn label(&self) -> String {
        if self.mime.starts_with("image/") {
            return format!("[{} · {} KiB]", self.mime, self.data.len().div_ceil(1024));
        }
        String::from_utf8_lossy(&self.data)
            .chars()
            .take(160)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    }
}
#[derive(Default)]
struct History {
    entries: Vec<Entry>,
}
impl History {
    fn insert(&mut self, entry: Entry) -> Result<bool> {
        entry.validate()?;
        if self.entries.first() == Some(&entry) {
            return Ok(false);
        }
        self.entries.retain(|old| old != &entry);
        self.entries.insert(0, entry);
        let mut bytes = 0;
        self.entries.truncate(MAX_ENTRIES);
        self.entries.retain(|entry| {
            bytes += entry.data.len();
            bytes <= MAX_TOTAL_BYTES
        });
        Ok(true)
    }
    fn append_legacy(&mut self, entries: Vec<Entry>) -> usize {
        let original = self.entries.len();
        let mut bytes = self
            .entries
            .iter()
            .map(|entry| entry.data.len())
            .sum::<usize>();
        let mut seen: HashSet<_> = self
            .entries
            .iter()
            .map(|entry| Sha256::digest(&entry.data))
            .collect();
        for entry in entries {
            if self.entries.len() == MAX_ENTRIES {
                break;
            }
            if entry.data.len() <= MAX_TOTAL_BYTES - bytes
                && seen.insert(Sha256::digest(&entry.data))
            {
                bytes += entry.data.len();
                self.entries.push(entry);
            }
        }
        self.entries.len() - original
    }
    fn write_to(&self, mut writer: impl Write) -> Result<()> {
        writer.write_all(MAGIC)?;
        writer.write_all(&(self.entries.len() as u32).to_le_bytes())?;
        for entry in &self.entries {
            entry.write_to(&mut writer, true)?;
        }
        Ok(())
    }
    fn read_from(mut reader: impl Read, blobs: &Path) -> Result<Self> {
        let mut header = [0; MAGIC.len()];
        reader.read_exact(&mut header)?;
        let blobs = match header.as_slice() {
            MAGIC => Some(blobs),
            LEGACY_MAGIC => None,
            _ => return Err(err("Invalid clipboard history header")),
        };
        let mut count = [0; 4];
        reader.read_exact(&mut count)?;
        let count = u32::from_le_bytes(count) as usize;
        if count > MAX_ENTRIES {
            return Err(err("Clipboard history entry limit exceeded"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut budget = MAX_TOTAL_BYTES;
        for _ in 0..count {
            let Some(entry) = Entry::read_from(&mut reader, budget, blobs)? else {
                continue;
            };
            budget -= entry.data.len();
            entries.push(entry);
        }
        if entries.iter().collect::<HashSet<_>>().len() != entries.len() {
            return Err(err("Duplicate clipboard history entry"));
        }
        if reader.read(&mut [0])? != 0 {
            return Err(err("Trailing clipboard history data"));
        }
        Ok(Self { entries })
    }
}
fn supported_mime(mime: &str) -> bool {
    matches!(
        mime,
        "text/plain"
            | "text/plain;charset=utf-8"
            | "text/plain;charset=UTF-8"
            | "UTF8_STRING"
            | "TEXT"
            | "STRING"
    ) || (mime.starts_with("image/")
        && mime.len() > 6
        && mime.len() <= 255
        && mime[6..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".+-_".contains(&b)))
}
fn preferred_mime(mimes: &[String]) -> Option<String> {
    if mimes.iter().any(|mime| {
        mime.eq_ignore_ascii_case("x-kde-passwordManagerHint")
            || mime == "application/x-keepassxc"
            || mime == "application/x-kde-passwordManagerHint"
    }) {
        return None;
    }
    [
        "image/png",
        "image/jpeg",
        "image/webp",
        "text/plain;charset=utf-8",
        "text/plain;charset=UTF-8",
        "UTF8_STRING",
        "text/plain",
        "TEXT",
        "STRING",
    ]
    .iter()
    .find_map(|preferred| {
        mimes
            .iter()
            .find(|mime| mime.as_str() == *preferred)
            .cloned()
    })
    .or_else(|| mimes.iter().find(|mime| supported_mime(mime)).cloned())
}
fn state_path() -> Result<PathBuf> {
    let directory = runtime::state_dir().join("hype-niri-clipboard");
    fs::create_dir_all(
        directory
            .parent()
            .ok_or_else(|| err("Missing state directory"))?,
    )?;
    private_dir(&directory)?;
    Ok(directory.join("history.bin"))
}
fn private_dir(directory: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    checked_dir(directory)
}
fn checked_dir(directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != runtime::uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(err("Unsafe clipboard history directory"));
    }
    Ok(())
}
fn private_file(path: &Path, limit: u64) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != runtime::uid()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > limit
    {
        return Err(err("Unsafe clipboard history file"));
    }
    Ok(Some(file))
}
fn load(path: &Path) -> Result<History> {
    let Some(file) = private_file(path, MAX_FILE_BYTES as u64)? else {
        return Ok(History::default());
    };
    History::read_from(BufReader::new(file), &blob_dir(path)?)
}
fn blob_dir(history: &Path) -> Result<PathBuf> {
    Ok(history
        .parent()
        .ok_or_else(|| err("Missing clipboard directory"))?
        .join("blobs"))
}
fn blob_name(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn is_blob_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
fn read_blob(blobs: &Path, name: &str, length: usize) -> Result<Vec<u8>> {
    if !is_blob_name(name) {
        return Err(err("Invalid clipboard blob reference"));
    }
    checked_dir(blobs)?;
    let mut file = private_file(&blobs.join(name), length as u64)?
        .ok_or_else(|| err("Missing clipboard blob"))?;
    let mut data = vec![0; length];
    file.read_exact(&mut data)?;
    if file.read(&mut [0])? != 0 || blob_name(&data) != name {
        return Err(err("Corrupt clipboard blob"));
    }
    Ok(data)
}
fn update(entry: Option<Entry>) -> Result<()> {
    let _lock = runtime::lock_blocking("clipboard-history.lock")?;
    let path = state_path()?;
    let Some(entry) = entry else {
        // A later watcher start must not repopulate a cleared history.
        mark_imported(&path)?;
        return save(&path, &History::default());
    };
    let mut history = load(&path)?;
    if !history.insert(entry)? {
        return Ok(());
    }
    save(&path, &history)
}
fn save(path: &Path, history: &History) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| err("Missing clipboard directory"))?;
    let blobs = blob_dir(path)?;
    private_dir(&blobs)?;
    let mut kept = HashSet::new();
    for entry in &history.entries {
        if entry.data.len() <= BLOB_BYTES {
            continue;
        }
        let name = blob_name(&entry.data);
        let target = blobs.join(&name);
        if read_blob(&blobs, &name, entry.data.len()).is_err() {
            let mut staged = tempfile::NamedTempFile::new_in(&blobs)?;
            staged
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
            staged.write_all(&entry.data)?;
            staged.as_file().sync_all()?;
            staged.persist(&target)?;
        }
        kept.insert(name);
    }
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        history.write_to(&mut writer)?;
        writer.flush()?;
    }
    let mut orphans = Vec::new();
    for file in fs::read_dir(&blobs)? {
        let name = file?.file_name();
        if let Some(name) = name.to_str()
            && is_blob_name(name)
            && !kept.contains(name)
        {
            orphans.push(blobs.join(name));
        }
    }
    if !orphans.is_empty() {
        staged.as_file().sync_all()?;
    }
    staged.persist(path)?;
    for orphan in orphans {
        fs::remove_file(orphan)?;
    }
    Ok(())
}
fn import_marker(history: &Path) -> PathBuf {
    history.with_file_name("cliphist-imported")
}
fn mark_imported(history: &Path) -> Result<()> {
    runtime::atomic_write(&import_marker(history), b"1\n")
}
fn snapshot() -> Result<History> {
    let _lock = runtime::lock_blocking("clipboard-history.lock")?;
    load(&state_path()?)
}
fn bounded_read(
    mut reader: impl Read + AsRawFd,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let fd = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(err("Clipboard offer read deadline exceeded"));
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut poll,
                1,
                remaining.as_millis().clamp(1, i32::MAX as u128) as i32,
            )
        };
        if ready < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.into());
        }
        if ready == 0 {
            return Err(err("Clipboard offer read deadline exceeded"));
        }
        let mut chunk = [0; 8192];
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(count) => {
                if count > limit.saturating_sub(bytes.len()) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Clipboard offer exceeds size limit",
                    )
                    .into());
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
}
fn legacy_output(
    command: &mut Command,
    limit: usize,
    deadline: Instant,
    capture_stderr: bool,
) -> Result<Option<Vec<u8>>> {
    let deadline = deadline.min(Instant::now() + READ_TIMEOUT);
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(if capture_stderr {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(if capture_stderr {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .process_group(0)
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let result = (|| {
        let output = if capture_stderr {
            child.stderr.take().map(OwnedFd::from)
        } else {
            child.stdout.take().map(OwnedFd::from)
        }
        .ok_or_else(|| err("Cannot read cliphist output"))?;
        let output = bounded_read(
            File::from(output),
            limit,
            deadline.saturating_duration_since(Instant::now()),
        )?;
        loop {
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(output);
                }
                return Err(err(format!("cliphist {status}")));
            }
            if Instant::now() >= deadline {
                return Err(err("cliphist import command timed out"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() && child.try_wait().ok().flatten().is_none() {
        // Also release stdout held by any child of the invoked utility.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
    }
    result.map(Some)
}
fn legacy_path(bytes: &[u8]) -> Option<PathBuf> {
    bytes.split(|byte| *byte == b'\n').find_map(|line| {
        let path = line.strip_prefix(b"db-path\t")?;
        let path = path.strip_suffix(b"\r").unwrap_or(path);
        (!path.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(path)))
    })
}
fn legacy_ids(list: &[u8]) -> impl Iterator<Item = &str> {
    list.split(|byte| *byte == b'\n').filter_map(|line| {
        let field = std::str::from_utf8(line.split(|byte| *byte == b'\t').next()?).ok()?;
        field.parse::<u64>().ok().map(|_| field)
    })
}
fn legacy_entry(data: Vec<u8>) -> Option<Entry> {
    let mime = match image::guess_format(&data) {
        Ok(format) => format.to_mime_type(),
        Err(_) if std::str::from_utf8(&data).is_ok() => "text/plain;charset=utf-8",
        Err(_) => return None,
    };
    let entry = Entry {
        mime: mime.into(),
        data,
    };
    entry.validate().ok().map(|()| entry)
}
fn import_legacy(explicit: bool, source: Option<&Path>) -> Result<Option<usize>> {
    let Some(_importing) = runtime::lock("cliphist-import.lock", Duration::ZERO)? else {
        if explicit {
            return Err(err("A cliphist import is already in progress"));
        }
        return Ok(None);
    };
    let history_path = state_path()?;
    if !explicit && import_marker(&history_path).exists() {
        return Ok(None);
    }
    let default_source = source.is_none();
    let deadline = Instant::now() + IMPORT_TIMEOUT;
    let source = match source {
        Some(source) => source.to_owned(),
        None => {
            // cliphist prints its version and effective flags to stderr.
            let Some(output) = legacy_output(
                Command::new("cliphist").arg("version"),
                8192,
                deadline,
                true,
            )?
            else {
                return if explicit {
                    Err(err("Install cliphist to import its old database"))
                } else {
                    Ok(None)
                };
            };
            legacy_path(&output).ok_or_else(|| err("cliphist did not report its database path"))?
        }
    };
    match fs::metadata(&source) {
        Ok(metadata) if metadata.is_file() && metadata.uid() == runtime::uid() => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => return Ok(None),
        _ => {
            return Err(err(
                "Expected a legacy clipboard database owned by this user",
            ));
        }
    }
    // The old utility opens list and decode in read-only mode. Pin both paths so a
    // cliphist setting cannot redirect either command to a different database.
    let run = |args: &[&str], limit| {
        legacy_output(
            Command::new("cliphist")
                .args([
                    "-config-path",
                    "/dev/null",
                    "-preview-width",
                    "0",
                    "-db-path",
                ])
                .arg(&source)
                .args(args)
                .env_remove("CLIPHIST_CONFIG_PATH")
                .env_remove("CLIPHIST_DB_PATH")
                .env_remove("CLIPHIST_PREVIEW_WIDTH"),
            limit,
            deadline,
            false,
        )?
        .ok_or_else(|| err("Install cliphist to import its old database"))
    };
    let list = run(&["list"], IMPORT_LIST_BYTES)?;
    let (remaining, mut bytes, mut seen) = {
        let history = snapshot()?;
        let bytes = history
            .entries
            .iter()
            .map(|entry| entry.data.len())
            .sum::<usize>();
        let seen = history
            .entries
            .iter()
            .map(|entry| Sha256::digest(&entry.data))
            .collect::<HashSet<_>>();
        (MAX_ENTRIES - history.entries.len(), bytes, seen)
    };
    let mut entries = Vec::new();
    for id in legacy_ids(&list).take(MAX_ENTRIES) {
        if entries.len() == remaining || bytes == MAX_TOTAL_BYTES {
            break;
        }
        if Instant::now() >= deadline {
            return Err(err("cliphist import timed out"));
        }
        let data = match run(&["decode", id], MAX_ENTRY_BYTES) {
            Ok(data) => data,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::InvalidData) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some(entry) = legacy_entry(data)
            && entry.data.len() <= MAX_TOTAL_BYTES - bytes
            && seen.insert(Sha256::digest(&entry.data))
        {
            bytes += entry.data.len();
            entries.push(entry);
        }
    }
    let _lock = runtime::lock_blocking("clipboard-history.lock")?;
    if !explicit && import_marker(&history_path).exists() {
        return Ok(None);
    }
    let mut history = load(&history_path)?;
    let count = history.append_legacy(entries);
    if count > 0 {
        save(&history_path, &history)?;
    }
    if default_source {
        mark_imported(&history_path)?;
    }
    Ok(Some(count))
}
async fn watch() -> Result<()> {
    let Some(_singleton) = runtime::lock("hype-clipboard-watch.lock", Duration::ZERO)? else {
        return Ok(());
    };
    // Create the protocol connection in its own thread: GTK's context owns signal handling.
    let (started, receive) = std::sync::mpsc::sync_channel(1);
    let (done, finished) = futures_channel::oneshot::channel::<()>();
    let worker = std::thread::spawn(move || -> std::result::Result<(), String> {
        let _done = done;
        let mut watcher = match Watcher::new(ClipboardType::Regular, paste::Seat::Unspecified) {
            Ok(watcher) => watcher,
            Err(error) => {
                let message = error.to_string();
                let _ = started.send(Err(message.clone()));
                return Err(message);
            }
        };
        let _ = started.send(Ok(watcher.cancel_handle()));
        while let Some(event) = watcher.next_event().map_err(|e| e.to_string())? {
            let ClipboardEvent::Changed {
                mime_types,
                mut offer,
                ..
            } = event
            else {
                continue;
            };
            let Some(mime) = preferred_mime(&mime_types) else {
                continue;
            };
            let saved = (|| -> Result<()> {
                let data = bounded_read(offer.receive(&mime)?, MAX_ENTRY_BYTES, READ_TIMEOUT)?;
                if !data.is_empty() {
                    update(Some(Entry { mime, data }))?;
                }
                Ok(())
            })();
            if let Err(error) = saved {
                eprintln!("Clipboard history skipped offer: {error}");
            }
        }
        Ok(())
    });
    let cancel = receive
        .recv()
        .map_err(|_| err("Clipboard watcher initialization failed"))?
        .map_err(err)?;
    let mut signals = Vec::new();
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let cancel = cancel.clone();
        signals.push(glib_unix::unix_signal_add_local(signal, move || {
            cancel.cancel();
            gtk::glib::ControlFlow::Continue
        }));
    }
    crate::watchers::ready(crate::watchers::Watcher::Clipboard)?;
    gtk::glib::spawn_future_local(async {
        if let Ok(Err(error)) = gtk::gio::spawn_blocking(|| {
            import_legacy(false, None).map_err(|error| error.to_string())
        })
        .await
        {
            eprintln!("Clipboard history import failed: {error}");
        }
    });
    let _ = finished.await;
    for source in signals {
        source.remove();
    }
    worker
        .join()
        .map_err(|_| err("Clipboard watcher panicked"))?
        .map_err(err)
}
fn start_owner(entry: &Entry) -> Result<()> {
    let token = format!("{OWNER_TOKEN_PREFIX}{:032x}", rand::random::<u128>());
    let (mut parent, child_socket) = UnixStream::pair()?;
    parent.set_read_timeout(Some(OWNER_TIMEOUT))?;
    parent.set_write_timeout(Some(OWNER_TIMEOUT))?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["clipboard", "owner", &token])
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(child_socket)))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let transfer = (|| -> Result<()> {
        entry.write_to(&mut parent, false)?;
        parent.shutdown(Shutdown::Write)?;
        let mut ready = [0; 1];
        parent.read_exact(&mut ready)?;
        if ready != [1] {
            return Err(err("Unexpected clipboard owner response"));
        }
        Ok(())
    })();
    match transfer {
        Ok(()) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(err(format!(
                "Clipboard owner did not confirm selection readiness: {error}"
            )))
        }
    }
}
fn owner(token: &str) -> Result<()> {
    if !token
        .strip_prefix(OWNER_TOKEN_PREFIX)
        .is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(err("Invalid clipboard owner request"));
    }
    // The selecting process passes one entry over a private inherited socket.
    let mut socket = UnixStream::from(std::io::stdin().as_fd().try_clone_to_owned()?);
    socket.set_read_timeout(Some(OWNER_TIMEOUT))?;
    socket.set_write_timeout(Some(OWNER_TIMEOUT))?;
    let entry = Entry::read_from(&mut socket, MAX_ENTRY_BYTES, None)?
        .ok_or_else(|| err("Invalid clipboard owner entry"))?;
    if socket.read(&mut [0])? != 0 {
        return Err(err("Trailing clipboard owner data"));
    }
    let mut watcher = Watcher::new(ClipboardType::Regular, paste::Seat::Unspecified)?;
    let cancel = watcher.cancel_handle();
    let token_seen = token.to_owned();
    let ready = std::thread::spawn(move || {
        while let Ok(Some(event)) = watcher.next_event() {
            if let ClipboardEvent::Changed { mime_types, .. } = event
                && mime_types.contains(&token_seen)
            {
                let _ = socket.write_all(&[1]);
                break;
            }
        }
    });
    let mut options = copy::Options::new();
    options
        .foreground(true)
        .omit_additional_text_mime_types(true);
    let result = options
        .prepare_copy_multi(vec![
            copy::MimeSource {
                source: copy::Source::Bytes(entry.data.into_boxed_slice()),
                mime_type: copy::MimeType::Specific(entry.mime),
            },
            copy::MimeSource {
                source: copy::Source::Bytes(Box::new([1])),
                mime_type: copy::MimeType::Specific(token.into()),
            },
        ])
        .and_then(|copy| copy.serve());
    cancel.cancel();
    let _ = ready.join();
    result?;
    Ok(())
}
pub async fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str).unwrap_or("pick") {
        "watch" if args.len() == 1 => watch().await,
        "clear" if args.len() == 1 => update(None),
        "import-cliphist" if args.len() <= 2 => {
            let count = import_legacy(true, args.get(1).map(Path::new))?.unwrap_or(0);
            println!("Imported {count} clipboard entries");
            Ok(())
        }
        "list" if args.len() == 1 => {
            for entry in snapshot()?.entries {
                println!("{}\t{}", entry.id(), entry.label());
            }
            Ok(())
        }
        "owner" if args.len() == 2 => owner(&args[1]),
        "copy" if args.len() == 2 => {
            let entry = snapshot()?
                .entries
                .into_iter()
                .find(|entry| entry.id() == args[1])
                .ok_or_else(|| err("Clipboard entry no longer exists"))?;
            start_owner(&entry)
        }
        "pick" if args.len() <= 1 => {
            let entries = snapshot()?.entries;
            if let Some(index) = dialog::choose(
                "Clipboard history",
                &entries.iter().map(Entry::label).collect::<Vec<_>>(),
            )
            .await?
            {
                start_owner(&entries[index])?;
            }
            Ok(())
        }
        _ => Err(err(
            "Usage: hype-control clipboard [watch|pick|clear|list|copy <id>|import-cliphist [database]]",
        )),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn encode(history: &History) -> Vec<u8> {
        let mut bytes = Vec::new();
        history.write_to(&mut bytes).unwrap();
        bytes
    }
    fn entry(mime: &str, data: &[u8]) -> Entry {
        Entry {
            mime: mime.into(),
            data: data.into(),
        }
    }
    #[test]
    fn total_storage_limit_removes_oldest_payloads() {
        let mut history = History::default();
        for value in 1..=5 {
            history
                .insert(entry("image/png", &vec![value; MAX_ENTRY_BYTES]))
                .unwrap();
        }
        assert_eq!(history.entries.len(), 4);
        assert_eq!(
            history
                .entries
                .iter()
                .map(|entry| entry.data.len())
                .sum::<usize>(),
            MAX_TOTAL_BYTES
        );
        assert_eq!(history.entries.last().unwrap().data[0], 2);
    }
    #[test]
    fn stalled_and_oversized_transfers_terminate_with_bounded_memory() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"12345").unwrap();
        assert!(
            bounded_read(reader, 4, Duration::from_millis(50))
                .unwrap_err()
                .to_string()
                .contains("size limit")
        );
        let (reader, _writer) = UnixStream::pair().unwrap();
        let began = Instant::now();
        assert!(
            bounded_read(reader, 4, Duration::from_millis(30))
                .unwrap_err()
                .to_string()
                .contains("deadline")
        );
        assert!(began.elapsed() < Duration::from_secs(1));
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"\0\xffPNG").unwrap();
        drop(writer);
        assert_eq!(
            bounded_read(reader, 16, Duration::from_millis(50)).unwrap(),
            b"\0\xffPNG"
        );
    }
    #[test]
    fn state_rejects_symlinks_public_permissions_and_malformed_bytes() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history");
        runtime::atomic_write(&path, &encode(&History::default())).unwrap();
        assert!(load(&path).is_ok());
        let link = temp.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, b"malformed").unwrap();
        assert!(load(&path).is_err());
        assert!(
            load(&temp.path().join("missing"))
                .unwrap()
                .entries
                .is_empty()
        );
    }
    #[test]
    fn raw_images_and_utf8_round_trip_without_reencoding() {
        let mut history = History::default();
        history
            .insert(entry("image/png", b"\x89PNG\0\xff"))
            .unwrap();
        history
            .insert(entry("text/plain;charset=utf-8", "👩🏽‍💻\0".as_bytes()))
            .unwrap();
        let saved = History::read_from(encode(&history).as_slice(), Path::new("blobs")).unwrap();
        assert_eq!(saved.entries, history.entries);
    }
    #[test]
    fn reads_existing_history_without_changing_its_format() {
        let fixture = b"HYPECLIP1\x01\0\0\0\x09\0\x06\0\0\0image/png\x89PNG\0\xff";
        let history = History::read_from(fixture.as_slice(), Path::new("blobs")).unwrap();
        assert_eq!(history.entries, [entry("image/png", b"\x89PNG\0\xff")]);
        let encoded = encode(&history);
        assert_eq!(&encoded[..MAGIC.len()], MAGIC);
        assert_eq!(&encoded[MAGIC.len()..], &fixture[MAGIC.len()..]);
        assert!(Entry::read_from(&fixture[13..], 5, None).is_err());
    }
    #[test]
    fn large_payloads_use_blobs_and_legacy_inline_history_still_loads() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history.bin");
        let blobs = temp.path().join("blobs");
        let large = entry("image/png", &vec![7; BLOB_BYTES + 1]);
        let other = entry("image/png", &vec![8; BLOB_BYTES + 1]);
        let mut legacy = LEGACY_MAGIC.to_vec();
        legacy.extend(1u32.to_le_bytes());
        large.write_to(&mut legacy, false).unwrap();
        runtime::atomic_write(&path, &legacy).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load(&path).unwrap().entries, std::slice::from_ref(&large));
        let mut history = History::default();
        history.insert(large.clone()).unwrap();
        history.insert(other.clone()).unwrap();
        history.insert(entry("text/plain", b"small")).unwrap();
        save(&path, &history).unwrap();
        assert!(fs::metadata(&path).unwrap().len() < 1024);
        let blob = blobs.join(blob_name(&large.data));
        assert_eq!(fs::metadata(&blob).unwrap().mode() & 0o777, 0o600);
        assert_eq!(load(&path).unwrap().entries, history.entries);
        fs::write(blobs.join("unrelated"), b"kept").unwrap();
        history.entries.retain(|entry| entry != &large);
        save(&path, &history).unwrap();
        assert!(!blob.exists());
        assert!(blobs.join(blob_name(&other.data)).exists());
        save(&path, &History::default()).unwrap();
        assert!(!blobs.join(blob_name(&other.data)).exists());
        assert!(blobs.join("unrelated").exists());
        fs::write(&blob, b"tampered").unwrap();
        history.entries = vec![large.clone()];
        save(&path, &history).unwrap();
        assert_eq!(load(&path).unwrap().entries, std::slice::from_ref(&large));
        let mut same_length = vec![9; BLOB_BYTES + 1];
        same_length[0] = 7;
        fs::write(&blob, &same_length).unwrap();
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&path).unwrap().entries.is_empty());
        save(&path, &history).unwrap();
        assert_eq!(load(&path).unwrap().entries, [large]);
        history.entries.insert(0, entry("text/plain", b"small"));
        save(&path, &history).unwrap();
        fs::remove_file(&blob).unwrap();
        assert_eq!(
            load(&path).unwrap().entries,
            [entry("text/plain", b"small")]
        );
    }
    #[test]
    fn duplicate_moves_to_front_and_retention_is_bounded() {
        let mut history = History::default();
        assert!(history.insert(entry("text/plain", b"old")).unwrap());
        history.insert(entry("text/plain", b"new")).unwrap();
        assert!(history.insert(entry("text/plain", b"old")).unwrap());
        assert!(!history.insert(entry("text/plain", b"old")).unwrap());
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].data, b"old");
        for value in 0..MAX_ENTRIES + 10 {
            history
                .insert(entry("text/plain", value.to_string().as_bytes()))
                .unwrap();
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert_eq!(
            history.entries[0].data,
            (MAX_ENTRIES + 9).to_string().as_bytes()
        );
    }
    #[test]
    fn legacy_cliphist_preserves_native_order_and_decodes_payload_types() {
        let version = b"version\t0.7.0\ndb-path\t/tmp/clipboard with spaces.db\n";
        assert_eq!(
            legacy_path(version).unwrap(),
            Path::new("/tmp/clipboard with spaces.db")
        );
        let list = b"10\tpreview\n9\t[[ binary data ]]\ninvalid row\n8\tmore\n";
        assert_eq!(legacy_ids(list).collect::<Vec<_>>(), ["10", "9", "8"]);
        assert!(legacy_entry(vec![0xff, 0xff]).is_none());
        let png = b"\x89PNG\r\n\x1a\nraw bytes";
        assert_eq!(legacy_entry(png.to_vec()).unwrap(), entry("image/png", png));
        let mut history = History::default();
        history
            .insert(entry("text/plain", b"already saved"))
            .unwrap();
        let legacy = || {
            [b"already saved".as_slice(), b"recent", b"older"]
                .into_iter()
                .map(|bytes| legacy_entry(bytes.to_vec()).unwrap())
                .collect()
        };
        assert_eq!(history.append_legacy(legacy()), 2);
        assert_eq!(history.append_legacy(legacy()), 0);
        assert_eq!(
            history
                .entries
                .iter()
                .map(|entry| entry.data.as_slice())
                .collect::<Vec<_>>(),
            [b"already saved".as_slice(), b"recent", b"older"]
        );
    }
    #[test]
    fn sensitive_hints_and_unsupported_mimes_are_never_saved() {
        let mimes = |names: &[&str]| names.iter().map(|s| (*s).into()).collect::<Vec<_>>();
        assert_eq!(
            preferred_mime(&mimes(&["text/plain", "x-kde-passwordManagerHint"])),
            None
        );
        assert_eq!(preferred_mime(&mimes(&["application/octet-stream"])), None);
        assert_eq!(
            preferred_mime(&mimes(&["text/plain", "image/png"])),
            Some("image/png".into())
        );
        assert_eq!(
            preferred_mime(&mimes(&["UTF8_STRING"])),
            Some("UTF8_STRING".into())
        );
    }
    #[test]
    fn malformed_state_and_oversized_entries_fail_closed() {
        assert!(History::read_from(b"{}".as_slice(), Path::new("blobs")).is_err());
        let mut history = History::default();
        assert!(
            history
                .insert(entry("image/png", &vec![1; 8 * 1024 * 1024 + 1]))
                .is_err()
        );
        assert!(
            history
                .insert(entry("text/plain\nimage/png", b"bad"))
                .is_err()
        );
        assert!(history.insert(entry("text/plain", b"ok")).is_ok());
        let mut bytes = encode(&history);
        bytes.push(0);
        assert!(History::read_from(bytes.as_slice(), Path::new("blobs")).is_err());
        bytes.truncate(12);
        assert!(History::read_from(bytes.as_slice(), Path::new("blobs")).is_err());
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    fn until(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(
                Instant::now() < deadline,
                "Isolated clipboard fixture timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn offer(mime: &str, data: &[u8], sensitive: bool) {
        let mut options = copy::Options::new();
        options
            .omit_additional_text_mime_types(true)
            .sensitive(sensitive);
        options
            .copy(
                copy::Source::Bytes(data.to_vec().into_boxed_slice()),
                copy::MimeType::Specific(mime.into()),
            )
            .unwrap();
    }
    fn contents(mime: &str) -> Vec<u8> {
        let (reader, actual) = paste::get_contents(
            paste::ClipboardType::Regular,
            paste::Seat::Unspecified,
            paste::MimeType::Specific(mime),
        )
        .unwrap();
        assert_eq!(actual, mime);
        bounded_read(reader, MAX_ENTRY_BYTES, READ_TIMEOUT).unwrap()
    }
    #[test]
    #[ignore = "requires HYPE_NATIVE_PROTOCOL_TESTS=1, private state/runtime and an isolated Wayland compositor"]
    fn native_watch_copy_image_sensitive_singleton_and_shutdown() {
        assert_eq!(
            std::env::var("HYPE_NATIVE_PROTOCOL_TESTS").as_deref(),
            Ok("1")
        );
        let binary = std::env::var_os("HYPE_CONTROL_TEST_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/hype-control")
            });
        let private_state =
            std::env::var_os("XDG_STATE_HOME").expect("Isolated state directory required");
        assert!(
            Path::new(&private_state).starts_with(std::env::temp_dir()),
            "Refuse to test against desktop history"
        );
        update(None).unwrap();
        let mut watcher = Child(
            Command::new(&binary)
                .args(["clipboard", "watch"])
                .spawn()
                .unwrap(),
        );
        let text = "private protocol fixture 👩🏽‍💻".as_bytes();
        offer("text/plain;charset=utf-8", text, false);
        until(|| {
            snapshot()
                .unwrap()
                .entries
                .first()
                .is_some_and(|entry| entry.data == text)
        });
        assert!(
            Command::new(&binary)
                .args(["clipboard", "watch"])
                .status()
                .unwrap()
                .success()
        );
        let image = b"\x89PNG\r\n\x1a\n\0\xffopaque binary fixture";
        offer("image/png", image, false);
        until(|| {
            snapshot()
                .unwrap()
                .entries
                .first()
                .is_some_and(|entry| entry.mime == "image/png" && entry.data == image)
        });
        let saved_image = snapshot().unwrap().entries[0].clone();
        offer("text/plain", b"sensitive fixture never retained", true);
        assert_eq!(contents("text/plain"), b"sensitive fixture never retained");
        offer(
            "text/plain;charset=utf-8",
            b"after sensitive fixture",
            false,
        );
        until(|| {
            snapshot()
                .unwrap()
                .entries
                .first()
                .is_some_and(|entry| entry.data == b"after sensitive fixture")
        });
        assert!(
            !snapshot()
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.data == b"sensitive fixture never retained")
        );
        let mut selected = Command::new(&binary)
            .args(["clipboard", "copy", &saved_image.id()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        until(|| selected.try_wait().unwrap().is_some());
        assert!(selected.wait().unwrap().success());
        // A retained owner must not keep callers' captured output pipes open.
        assert!(
            bounded_read(
                selected.stdout.take().unwrap(),
                4096,
                Duration::from_millis(500)
            )
            .is_ok()
        );
        assert!(
            bounded_read(
                selected.stderr.take().unwrap(),
                4096,
                Duration::from_millis(500)
            )
            .is_ok()
        );
        assert_eq!(contents("image/png"), image);
        // The selecting process has exited; the retained owner must still serve multiple requests.
        assert_eq!(contents("image/png"), image);
        until(|| snapshot().unwrap().entries[0] == saved_image);
        assert_eq!(
            snapshot()
                .unwrap()
                .entries
                .iter()
                .filter(|entry| **entry == saved_image)
                .count(),
            1
        );
        unsafe {
            libc::kill(watcher.0.id() as i32, libc::SIGTERM);
        }
        until(|| watcher.0.try_wait().unwrap().is_some());
        assert!(watcher.0.wait().unwrap().success());
        copy::clear(copy::ClipboardType::Regular, copy::Seat::All).unwrap();
    }
}
