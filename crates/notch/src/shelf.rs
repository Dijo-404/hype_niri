use gtk::{gdk, gdk_pixbuf, gio, glib, prelude::*};
use std::{
    cell::OnceCell,
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime},
};

pub const LIFETIME: Duration = Duration::from_secs(10 * 60);
const ITEM_PREFIX: &str = "item-";

pub struct CachedFile {
    path: PathBuf,
    source: Option<PathBuf>,
    _directory: tempfile::TempDir,
    _lock: fs::File,
    pub thumbnail: OnceCell<Option<gdk::Texture>>,
}

fn open_directory(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

pub fn prune(root: &Path, now: SystemTime) -> io::Result<()> {
    let root_lock = match open_directory(root) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    root_lock.lock()?;
    for item in fs::read_dir(root)? {
        let item = item?;
        if let Err(error) = prune_item(item, now)
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
    }
    Ok(())
}

fn prune_item(item: fs::DirEntry, now: SystemTime) -> io::Result<()> {
    if !item.file_type()?.is_dir() {
        return Ok(());
    }
    let name = item.file_name();
    let Some(name) = name.to_str() else {
        return Ok(());
    };
    if let Some(suffix) = name.strip_prefix(".tmp")
        && suffix.len() == 6
        && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && now
            .duration_since(item.metadata()?.modified()?)
            .is_ok_and(|age| age >= LIFETIME)
    {
        // Earlier versions left unmarked directories and used the same ten-minute lifetime.
        fs::remove_dir_all(item.path())?;
    } else if name.starts_with(ITEM_PREFIX) {
        let lock = open_directory(&item.path())?;
        match lock.try_lock() {
            Ok(()) => fs::remove_dir_all(item.path())?,
            Err(std::fs::TryLockError::WouldBlock) => (),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(())
}

impl CachedFile {
    fn directory(root: &Path) -> io::Result<(tempfile::TempDir, fs::File)> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        let root_lock = open_directory(root)?;
        root_lock.lock()?;
        let item = tempfile::Builder::new()
            .prefix(ITEM_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(root)?;
        let lock = open_directory(item.path())?;
        lock.lock()?;
        Ok((item, lock))
    }

    pub fn copy(root: &Path, source: PathBuf) -> io::Result<Self> {
        let Some(name) = source.file_name().filter(|_| source.is_file()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a local file",
            ));
        };
        let (directory, lock) = Self::directory(root)?;
        let path = directory.path().join(name);
        fs::copy(&source, &path)?;
        Ok(Self {
            path,
            source: Some(source),
            _directory: directory,
            _lock: lock,
            thumbnail: OnceCell::new(),
        })
    }

    pub fn screenshot(root: &Path, texture: gdk::Texture) -> io::Result<Self> {
        let time = glib::DateTime::now_local()
            .and_then(|time| time.format("%H-%M-%S-%f"))
            .map_err(io::Error::other)?;
        let (directory, lock) = Self::directory(root)?;
        let path = directory.path().join(format!("Screenshot {time}.png"));
        fs::write(&path, texture.save_to_png_bytes())?;
        Ok(Self {
            path,
            source: None,
            _directory: directory,
            _lock: lock,
            thumbnail: OnceCell::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("hype-niri/notch-shelf")
}

pub async fn thumbnail(file: gio::File) -> Option<gdk::MemoryTexture> {
    let stream = file.read_future(glib::Priority::DEFAULT).await.ok()?;
    let pixels = gdk_pixbuf::Pixbuf::from_stream_at_scale_future(&stream, 52, 38, true)
        .await
        .ok()?;
    let format = if pixels.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    Some(gdk::MemoryTexture::new(
        pixels.width(),
        pixels.height(),
        format,
        &pixels.read_pixel_bytes(),
        pixels.rowstride() as usize,
    ))
}

#[derive(Default)]
pub struct Shelf(Vec<(Rc<CachedFile>, Instant)>);

impl Shelf {
    pub fn add(&mut self, files: Vec<CachedFile>, now: Instant) {
        for file in files {
            let existing = file.source.as_ref().and_then(|source| {
                self.0
                    .iter_mut()
                    .find(|(item, _)| item.source.as_ref() == Some(source))
            });
            let entry = (Rc::new(file), now + LIFETIME);
            if let Some(existing) = existing {
                *existing = entry;
            } else {
                self.0.push(entry);
            }
        }
    }

    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.0.len();
        self.0.retain(|(file, _)| file.path() != path);
        self.0.len() != before
    }

    pub fn expire(&mut self, now: Instant) {
        self.0.retain(|(_, expires)| *expires > now);
    }

    pub fn next_expiry(&self) -> Option<Instant> {
        self.0.iter().map(|(_, expires)| *expires).min()
    }

    pub fn files(&self) -> Vec<Rc<CachedFile>> {
        self.0.iter().map(|(file, _)| file.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pruning_keeps_live_copies_and_clears_unlocked_caches() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("cache");
        let source = fixture.path().join("original");
        fs::write(&source, "keep").unwrap();
        let live = CachedFile::copy(&root, source).unwrap();
        let orphan = root.join("item-dead");
        let legacy = root.join(".tmpabc123");
        let unrelated = root.join("personal");
        for path in [&orphan, &legacy, &unrelated] {
            fs::create_dir(path).unwrap();
        }
        prune(&root, SystemTime::now() + LIFETIME).unwrap();
        assert!(live.path().is_file());
        assert!(!orphan.exists());
        assert!(!legacy.exists());
        assert!(unrelated.is_dir());
    }

    #[test]
    fn shelf_caches_copies_and_removes_only_expired_cache_files() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("cache");
        let sources: Vec<_> = (1..=5)
            .map(|index| fixture.path().join(format!("shot {index}.png")))
            .collect();
        for (index, source) in sources.iter().enumerate() {
            fs::write(source, [index as u8]).unwrap();
        }
        let mut shelf = Shelf::default();
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        let copy = |source: &PathBuf| CachedFile::copy(&root, source.clone()).unwrap();
        shelf.add(sources.iter().map(copy).collect(), start);
        let first_path = shelf.files()[0].path().to_path_buf();
        fs::write(&sources[0], [100]).unwrap();
        assert_eq!(fs::read(&first_path).unwrap(), [0]);
        shelf.add(vec![copy(&sources[0])], start + minute);
        assert!(!first_path.exists());
        assert_eq!(shelf.files().len(), 5);
        assert_eq!(shelf.next_expiry(), Some(start + LIFETIME));
        shelf.expire(start + LIFETIME);
        assert_eq!(shelf.files().len(), 1);
        let last_path = shelf.files()[0].path().to_path_buf();
        assert_eq!(fs::read(&last_path).unwrap(), [100]);
        assert_eq!(shelf.next_expiry(), Some(start + LIFETIME + minute));
        shelf.expire(start + LIFETIME + minute);
        assert!(shelf.files().is_empty());
        assert!(!last_path.exists());
        assert!(sources.iter().all(|source| source.is_file()));
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);
    }
}
