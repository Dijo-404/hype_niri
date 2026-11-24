use crate::{Result, dialog, err, runtime};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    net::Shutdown,
    os::{
        fd::{AsFd, AsRawFd},
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::UnixStream,
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

const MAX_ENTRIES: usize = 64;
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILE_BYTES: usize = MAX_TOTAL_BYTES + MAX_ENTRIES * 300 + 16;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_TIMEOUT: Duration = Duration::from_secs(4);
const OWNER_TOKEN_PREFIX: &str = "application/x-hype-clipboard-";
const MAGIC: &[u8] = b"HYPECLIP1";
#[derive(Clone, Debug, PartialEq, Eq)]
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
    fn write_to(&self, mut writer: impl Write) -> Result<()> {
        self.validate()?;
        writer.write_all(&(self.mime.len() as u16).to_le_bytes())?;
        writer.write_all(&(self.data.len() as u32).to_le_bytes())?;
        writer.write_all(self.mime.as_bytes())?;
        writer.write_all(&self.data)?;
        Ok(())
    }
    fn read_from(mut reader: impl Read, budget: usize) -> Result<Self> {
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
        let mut data = vec![0; data_length];
        reader.read_exact(&mut data)?;
        Ok(Self { mime, data })
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
    fn write_to(&self, mut writer: impl Write) -> Result<()> {
        writer.write_all(MAGIC)?;
        writer.write_all(&(self.entries.len() as u32).to_le_bytes())?;
        for entry in &self.entries {
            entry.write_to(&mut writer)?;
        }
        Ok(())
    }
    fn read_from(mut reader: impl Read) -> Result<Self> {
        let mut header = [0; MAGIC.len()];
        reader.read_exact(&mut header)?;
        if header.as_slice() != MAGIC {
            return Err(err("Invalid clipboard history header"));
        }
        let mut count = [0; 4];
        reader.read_exact(&mut count)?;
        let count = u32::from_le_bytes(count) as usize;
        if count > MAX_ENTRIES {
            return Err(err("Clipboard history entry limit exceeded"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut budget = MAX_TOTAL_BYTES;
        for _ in 0..count {
            let entry = Entry::read_from(&mut reader, budget)?;
            budget -= entry.data.len();
            if entries.contains(&entry) {
                return Err(err("Duplicate clipboard history entry"));
            }
            entries.push(entry);
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
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != runtime::uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(err("Unsafe clipboard history directory"));
    }
    Ok(directory.join("history.bin"))
}
fn load(path: &Path) -> Result<History> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(History::default()),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != runtime::uid()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > MAX_FILE_BYTES as u64
    {
        return Err(err("Unsafe clipboard history file"));
    }
    History::read_from(BufReader::new(file))
}
fn update(entry: Option<Entry>) -> Result<()> {
    let _lock = runtime::lock_blocking("clipboard-history.lock")?;
    let path = state_path()?;
    let mut history = load(&path)?;
    let changed = match entry {
        Some(entry) => history.insert(entry)?,
        None => !std::mem::take(&mut history.entries).is_empty(),
    };
    if !changed {
        return Ok(());
    }
    let directory = path
        .parent()
        .ok_or_else(|| err("Missing clipboard directory"))?;
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        history.write_to(&mut writer)?;
        writer.flush()?;
    }
    staged.as_file().sync_all()?;
    staged.persist(&path)?;
    Ok(())
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
                    return Err(err("Clipboard offer exceeds size limit"));
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
async fn watch() -> Result<()> {
    let Some(_singleton) = runtime::lock("clipboard-watcher.lock", Duration::ZERO)? else {
        return Ok(());
    };
    // Create the protocol connection in its own thread: GTK's context owns signal handling.
    let (started, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || -> std::result::Result<(), String> {
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
    while !worker.is_finished() {
        gtk::glib::timeout_future(Duration::from_millis(100)).await;
    }
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
        entry.write_to(&mut parent)?;
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
    let entry = Entry::read_from(&mut socket, MAX_ENTRY_BYTES)?;
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
            )? {
                start_owner(&entries[index])?;
            }
            Ok(())
        }
        _ => Err(err(
            "Usage: hype-control clipboard [watch|pick|clear|list|copy <id>]",
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
        let saved = History::read_from(encode(&history).as_slice()).unwrap();
        assert_eq!(saved.entries, history.entries);
    }
    #[test]
    fn reads_existing_history_without_changing_its_format() {
        let fixture = b"HYPECLIP1\x01\0\0\0\x09\0\x06\0\0\0image/png\x89PNG\0\xff";
        let history = History::read_from(fixture.as_slice()).unwrap();
        assert_eq!(history.entries, [entry("image/png", b"\x89PNG\0\xff")]);
        assert_eq!(encode(&history), fixture);
        assert!(Entry::read_from(&fixture[13..], 5).is_err());
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
        for value in 0..100u8 {
            history.insert(entry("text/plain", &[value])).unwrap();
        }
        assert_eq!(history.entries.len(), 64);
        assert_eq!(history.entries[0].data, [99]);
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
        assert!(History::read_from(b"{}".as_slice()).is_err());
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
        assert!(History::read_from(bytes.as_slice()).is_err());
        bytes.truncate(12);
        assert!(History::read_from(bytes.as_slice()).is_err());
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
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/hype-control")
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
