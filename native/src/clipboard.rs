use crate::{Result, dialog, err, runtime};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
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
    fn insert(&mut self, entry: Entry) -> Result<()> {
        entry.validate()?;
        self.entries.retain(|old| old != &entry);
        self.entries.insert(0, entry);
        let mut bytes = 0;
        self.entries.truncate(MAX_ENTRIES);
        self.entries.retain(|entry| {
            bytes += entry.data.len();
            bytes <= MAX_TOTAL_BYTES
        });
        Ok(())
    }
    fn encode(&self) -> Result<Vec<u8>> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            entry.validate()?;
            bytes.extend_from_slice(&(entry.mime.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(entry.mime.as_bytes());
            bytes.extend_from_slice(&entry.data);
        }
        Ok(bytes)
    }
    fn decode(mut bytes: &[u8]) -> Result<Self> {
        fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
            if length > bytes.len() {
                return Err(err("Truncated clipboard history"));
            }
            let (value, rest) = bytes.split_at(length);
            *bytes = rest;
            Ok(value)
        }
        if bytes.len() > MAX_FILE_BYTES || take(&mut bytes, MAGIC.len())? != MAGIC {
            return Err(err("Invalid clipboard history header"));
        }
        let count = u32::from_le_bytes(take(&mut bytes, 4)?.try_into()?) as usize;
        if count > MAX_ENTRIES {
            return Err(err("Clipboard history entry limit exceeded"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut total = 0;
        for _ in 0..count {
            let mime_length = u16::from_le_bytes(take(&mut bytes, 2)?.try_into()?) as usize;
            let data_length = u32::from_le_bytes(take(&mut bytes, 4)?.try_into()?) as usize;
            if mime_length > 255 || data_length > MAX_ENTRY_BYTES {
                return Err(err("Clipboard history size limit exceeded"));
            }
            total += data_length;
            if total > MAX_TOTAL_BYTES {
                return Err(err("Clipboard history total limit exceeded"));
            }
            let mime = std::str::from_utf8(take(&mut bytes, mime_length)?)?.to_owned();
            let data = take(&mut bytes, data_length)?.to_vec();
            let entry = Entry { mime, data };
            entry.validate()?;
            if entries.contains(&entry) {
                return Err(err("Duplicate clipboard history entry"));
            }
            entries.push(entry);
        }
        if !bytes.is_empty() {
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
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    History::decode(&bytes)
}
fn update(entry: Option<Entry>) -> Result<()> {
    let _lock = runtime::lock_blocking("clipboard-history.lock")?;
    let path = state_path()?;
    let mut history = load(&path)?;
    if let Some(entry) = entry {
        history.insert(entry)?;
    } else {
        history.entries.clear();
    }
    runtime::atomic_write(&path, &history.encode()?)
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
    let token = format!(
        "application/x-hype-clipboard-{:032x}",
        rand::random::<u128>()
    );
    let (mut parent, child_socket) = UnixStream::pair()?;
    parent.set_read_timeout(Some(Duration::from_secs(4)))?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["clipboard", "owner", &entry.id(), &token])
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(child_socket)))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut ready = [0; 1];
    match parent.read_exact(&mut ready) {
        Ok(()) if ready == [1] => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Err(err("Clipboard owner did not confirm selection readiness"))
        }
    }
}
fn owner(id: &str, token: &str) -> Result<()> {
    if id.len() != 64
        || !id.bytes().all(|b| b.is_ascii_hexdigit())
        || !token.starts_with("application/x-hype-clipboard-")
        || token.len() != 61
        || !token[29..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(err("Invalid clipboard owner request"));
    }
    let entry = snapshot()?
        .entries
        .into_iter()
        .find(|entry| entry.id() == id)
        .ok_or_else(|| err("Clipboard entry no longer exists"))?;
    let mut watcher = Watcher::new(ClipboardType::Regular, paste::Seat::Unspecified)?;
    let cancel = watcher.cancel_handle();
    let token_seen = token.to_owned();
    let ready = std::thread::spawn(move || {
        while let Ok(Some(event)) = watcher.next_event() {
            if let ClipboardEvent::Changed { mime_types, .. } = event
                && mime_types.contains(&token_seen)
            {
                // stdin is a private inherited socket, never the user's terminal.
                let mut socket = unsafe { UnixStream::from_raw_fd(0) };
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
        "owner" if args.len() == 3 => owner(&args[1], &args[2]),
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
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history");
        runtime::atomic_write(&path, &History::default().encode().unwrap()).unwrap();
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
        let saved = History::decode(&history.encode().unwrap()).unwrap();
        assert_eq!(saved.entries, history.entries);
    }
    #[test]
    fn duplicate_moves_to_front_and_retention_is_bounded() {
        let mut history = History::default();
        history.insert(entry("text/plain", b"old")).unwrap();
        history.insert(entry("text/plain", b"new")).unwrap();
        history.insert(entry("text/plain", b"old")).unwrap();
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
        assert!(History::decode(b"{}").is_err());
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
        let mut bytes = history.encode().unwrap();
        bytes.push(0);
        assert!(History::decode(&bytes).is_err());
        bytes.truncate(12);
        assert!(History::decode(&bytes).is_err());
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
