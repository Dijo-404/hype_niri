//! Wallpaper state and awww 0.12.1 IPC; no CLI image/control subprocesses.
use crate::{Result, err, runtime};
use awww_common::{
    compression::Compressor,
    ipc::{
        self, Answer, BgInfo, Coord, ImageRequestBuilder, ImgSend, IpcSocket, Nanos, PixelFormat,
        Position, RequestSend, Transition, TransitionType,
    },
};
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader, imageops::FilterType,
};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Cursor,
    num::NonZeroU8,
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt, fs::symlink},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, UNIX_EPOCH},
};

const MAX_ANIMATION_FRAMES: usize = 600;
const MAX_ANIMATION_FRAME_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ANIMATION_WORK_BYTES: u64 = 1024 * 1024 * 1024;

fn wallpaper_dir() -> Result<PathBuf> {
    Ok(std::env::var_os("WALLPAPER_DIR")
        .map(PathBuf::from)
        .unwrap_or(runtime::home()?.join("pictures/wallpapers")))
}
fn current_link() -> PathBuf {
    runtime::state_dir().join("niri/current_wallpaper")
}
fn collect_images(dir: &Path, images: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect_images(&path, images)?;
        } else if kind.is_file()
            && path.extension().is_some_and(|s| {
                [b"png".as_slice(), b"jpg", b"jpeg", b"webp"]
                    .iter()
                    .any(|ext| s.as_bytes().eq_ignore_ascii_case(ext))
            })
        {
            images.push(path);
        }
    }
    Ok(())
}
fn images(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    collect_images(dir, &mut images)?;
    images.sort();
    Ok(images)
}
fn update_link(
    link: &Path,
    image: &Path,
    accept: impl FnOnce(&Path) -> Result<bool>,
) -> Result<()> {
    let image = image.canonicalize()?;
    if !image.is_file() {
        return Err(err("Wallpaper must be a regular image file"));
    }
    let dir = link
        .parent()
        .ok_or_else(|| err("Wallpaper state has no parent"))?;
    fs::create_dir_all(dir)?;
    let temporary = tempfile::Builder::new()
        .prefix(".current-wallpaper.")
        .tempdir_in(dir)?;
    let staged = temporary.path().join("link");
    symlink(&image, &staged)?;
    if accept(&image)? {
        fs::rename(staged, link)?;
    }
    Ok(())
}
fn ensure(link: &Path, dir: &Path) -> Result<()> {
    if link.is_file() {
        return Ok(());
    }
    let Some(first) = images(dir)?.into_iter().next() else {
        return Ok(());
    };
    update_link(link, &first, |_| Ok(true))
}
fn decode(path: &Path) -> Result<DynamicImage> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}
fn thumbnail(path: &Path, cache: &Path) -> Result<PathBuf> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().as_bytes());
    hash.update([0]);
    hash.update(modified.as_nanos().to_ne_bytes());
    hash.update(metadata.len().to_ne_bytes());
    let destination = cache.join(format!("{:x}.png", hash.finalize()));
    if destination.metadata().is_ok_and(|m| m.len() > 0) {
        return Ok(destination);
    }
    fs::create_dir_all(cache)?;
    let image = decode(path)?.thumbnail(256, 256);
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, ImageFormat::Png)?;
    runtime::atomic_write(&destination, encoded.get_ref())?;
    Ok(destination)
}
fn label(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() })));
    let wayland = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into());
    let mut name = Path::new(&wayland)
        .file_name()
        .unwrap_or(&wayland)
        .to_os_string();
    name.push("-awww-daemon.sock");
    runtime.join(name)
}
fn connect_at(path: &Path) -> Result<IpcSocket> {
    let stream = crate::ipc::connect_timeout(path, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let fd: OwnedFd = stream.into();
    Ok(IpcSocket::new(fd))
}
fn daemon_ready() -> Result<bool> {
    match request_at(&socket_path(), RequestSend::Ping)? {
        Answer::Ping(ready) => Ok(ready),
        _ => Err(err("awww returned an invalid readiness response")),
    }
}
fn ensure_daemon() -> Result<()> {
    let mut child = match daemon_ready() {
        Ok(true) => return Ok(()),
        Ok(false) => None,
        Err(error)
            if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                )
            }) =>
        {
            Some(
                Command::new("awww-daemon")
                    .args(["--format", "xrgb"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?,
            )
        }
        Err(error) => return Err(error),
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if daemon_ready().unwrap_or(false) {
            return Ok(());
        }
        if let Some(child) = child.as_mut()
            && let Some(status) = child.try_wait()?
        {
            return Err(err(format!("awww-daemon exited: {status}")));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(err("awww-daemon did not become ready"))
}
fn request_at(socket: &Path, request: RequestSend) -> Result<Answer> {
    let socket = connect_at(socket)?;
    request.send(&socket).map_err(|e| err(e.to_string()))?;
    Ok(Answer::receive(
        socket.recv().map_err(|e| err(e.to_string()))?,
    ))
}
fn transition(animate: bool) -> Transition {
    // Upstream maps the CLI's `center` to Grow at the center position.
    Transition {
        transition_type: if animate {
            TransitionType::Grow
        } else {
            TransitionType::None
        },
        duration: 0.18,
        step: NonZeroU8::new(90).unwrap(),
        fps: 60,
        angle: 45.,
        pos: Position::new(Coord::Percent(0.5), Coord::Percent(0.5)),
        bezier: (0.333333, 0.666667, 0.666667, 1.),
        wave: (20., 20.),
        invert_y: false,
    }
}
fn check_output_dimensions(dim: (u32, u32)) -> Result<()> {
    if dim.0 == 0 || dim.1 == 0 || u64::from(dim.0) * u64::from(dim.1) > 134_217_728 {
        return Err(err("Invalid or excessive wallpaper output dimensions"));
    }
    Ok(())
}
fn pixels(image: &DynamicImage, dim: (u32, u32), format: PixelFormat) -> Result<Box<[u8]>> {
    check_output_dimensions(dim)?;
    let resized = image.resize_to_fill(dim.0, dim.1, FilterType::Lanczos3);
    let mut bytes = if format.channels() == 3 {
        resized.into_rgb8().into_raw()
    } else {
        resized.into_rgba8().into_raw()
    };
    if format.must_swap_r_and_b_channels() {
        for pixel in bytes.chunks_exact_mut(format.channels() as usize) {
            pixel.swap(0, 2);
        }
    }
    Ok(bytes.into_boxed_slice())
}
fn animation(
    path: &Path,
    dim: (u32, u32),
    format: PixelFormat,
    budget: &mut u64,
) -> Result<Option<ipc::Animation>> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let source_format = reader.format();
    let file = reader.into_inner();
    let (source_dim, mut frames) = match source_format {
        Some(ImageFormat::WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(file)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            (decoder.dimensions(), decoder.into_frames())
        }
        Some(ImageFormat::Png) => {
            let decoder =
                image::codecs::png::PngDecoder::with_limits(file, image::Limits::default())?;
            if !decoder.is_apng()? {
                return Ok(None);
            }
            (decoder.dimensions(), decoder.apng()?.into_frames())
        }
        Some(ImageFormat::Gif) => {
            let decoder = image::codecs::gif::GifDecoder::new(file)?;
            (decoder.dimensions(), decoder.into_frames())
        }
        _ => return Ok(None),
    };
    let decoded_bytes = (u64::from(source_dim.0) * u64::from(source_dim.1)).saturating_mul(4);
    let target_pixels = u64::from(dim.0) * u64::from(dim.1);
    if decoded_bytes > MAX_ANIMATION_FRAME_BYTES || target_pixels > MAX_ANIMATION_FRAME_BYTES / 4 {
        return Err(err("Wallpaper animation frames are too large"));
    }
    // Count raw frame work and allow for awww's changed-pixel run markers.
    let frame_cost = decoded_bytes + target_pixels * 5;
    let mut charge_frame = || -> Result<()> {
        *budget = budget
            .checked_sub(frame_cost)
            .ok_or_else(|| err("Wallpaper animation exceeds the request's processing budget"))?;
        Ok(())
    };
    charge_frame()?;
    let Some(first) = frames.next() else {
        return Ok(None);
    };
    let first = first?;
    let delay = |frame: &image::Frame| {
        let (n, d) = frame.delay().numer_denom_ms();
        Nanos::from_nanos((u64::from(n) * 1_000_000) / u64::from(d.max(1)))
    };
    let mut first_duration = delay(&first);
    // awww compresses RGB triplets even when the daemon uses a four-channel framebuffer.
    let animation_format = if format.must_swap_r_and_b_channels() {
        PixelFormat::Rgb
    } else {
        PixelFormat::Bgr
    };
    let render = |frame: image::Frame| {
        pixels(
            &DynamicImage::ImageRgba8(frame.into_buffer()),
            dim,
            animation_format,
        )
    };
    let first = render(first)?;
    let mut previous = None;
    let mut compressor = Compressor::new();
    let mut compressed: Vec<(_, Nanos)> = Vec::new();
    for (index, frame) in frames.enumerate() {
        if index >= MAX_ANIMATION_FRAMES - 1 {
            return Err(err("Wallpaper animation has too many frames"));
        }
        charge_frame()?;
        let frame = frame?;
        let duration = delay(&frame);
        let current = render(frame)?;
        let prior = previous.as_deref().unwrap_or(first.as_ref());
        if let Some(pack) = compressor.compress(prior, &current, format) {
            compressed.push((pack, duration));
        } else if let Some(last) = compressed.last_mut() {
            last.1 += duration;
        } else {
            first_duration += duration;
        }
        previous = Some(current);
    }
    if let Some(previous) = previous {
        if let Some(pack) = compressor.compress(&previous, &first, format) {
            compressed.push((pack, first_duration));
        } else if let Some(last) = compressed.last_mut() {
            last.1 += first_duration;
        }
    }
    if compressed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ipc::Animation {
            animation: compressed.into_boxed_slice(),
        }))
    }
}

struct OutputGroup {
    dim: (u32, u32),
    format: PixelFormat,
    names: Vec<String>,
}

fn group_outputs(outputs: &[BgInfo]) -> Vec<OutputGroup> {
    let mut groups: Vec<OutputGroup> = Vec::new();
    for output in outputs {
        let dim = output.real_dim();
        let format = output.pixel_format;
        let name = output.name.to_string();
        if let Some(group) = groups
            .iter_mut()
            .find(|g| g.dim == dim && g.format == format)
        {
            group.names.push(name);
        } else {
            groups.push(OutputGroup {
                dim,
                format,
                names: vec![name],
            });
        }
    }
    groups
}

fn build_request(
    path: &Path,
    outputs: &[BgInfo],
    animate: bool,
) -> Result<awww_common::mmap::Mmap> {
    let path = path.canonicalize()?;
    let path_string = path
        .to_str()
        .ok_or_else(|| err("awww requires UTF-8 image paths"))?
        .to_owned();
    if outputs.len() > u8::MAX as usize {
        return Err(err("Too many wallpaper outputs"));
    }
    let image = decode(&path)?;
    let mut budget = MAX_ANIMATION_WORK_BYTES;
    let groups = group_outputs(outputs);
    let mut prepared = Vec::with_capacity(groups.len());
    for group in groups {
        check_output_dimensions(group.dim)?;
        let animation = animation(&path, group.dim, group.format, &mut budget)?;
        prepared.push((group, animation));
    }
    let mut builder = ImageRequestBuilder::new(transition(animate))?;
    for (OutputGroup { dim, format, names }, animation) in prepared {
        builder.push(
            ImgSend {
                path: path_string.clone(),
                dim,
                format,
                img: pixels(&image, dim, format)?,
            },
            "",
            "crop",
            None,
            "Lanczos3",
            &names,
            animation,
        );
    }
    Ok(builder.build())
}
fn apply(path: &Path, animate: bool) -> Result<bool> {
    ensure_daemon()?;
    let socket = socket_path();
    let outputs = match request_at(&socket, RequestSend::Query)? {
        Answer::Info(outputs) => outputs,
        _ => return Err(err("awww returned an invalid output query response")),
    };
    if outputs.is_empty() {
        return Ok(false);
    }
    let request = build_request(path, &outputs, animate)?;
    match request_at(&socket, RequestSend::Img(request))? {
        Answer::Ok => Ok(true),
        _ => Err(err("awww did not accept the wallpaper")),
    }
}
pub async fn run(command: &str, args: &[String]) -> Result<()> {
    if command != "wallpaper" {
        return Err(err(format!("Unknown wallpaper command: {command}")));
    }
    let Some(guard) = runtime::lock("hype-wallpaper.lock", Duration::from_secs(2))? else {
        return Ok(());
    };
    let dir = wallpaper_dir()?;
    let current = current_link();
    let action = args.first().map(String::as_str).unwrap_or("");
    match action {
        "ensure" => {
            ensure(&current, &dir)?;
        }
        "init" | "current" | "restore" | "sync" => {
            if current.is_file() {
                apply(&current, false)?;
            } else if let Some(first) = images(&dir)?.into_iter().next() {
                update_link(&current, &first, |path| apply(path, false))?;
            }
        }
        "random" => {
            let images = images(&dir)?;
            if !images.is_empty() {
                let image = &images[rand::rng().random_range(0..images.len())];
                update_link(&current, image, |path| apply(path, true))?;
            }
        }
        "select" => {
            let images = images(&dir)?;
            if images.is_empty() {
                return Ok(());
            }
            drop(guard);
            let cache = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or(runtime::home()?.join(".cache"))
                .join("hype-niri/wallpapers");
            let items: Vec<_> = images.iter().map(|p| (label(p), p.clone())).collect();
            let Some(index) = crate::dialog::images_lazy("Wallpaper", &items, move |path| {
                thumbnail(path, &cache).ok()
            })?
            else {
                return Ok(());
            };
            let image = images
                .get(index)
                .ok_or_else(|| err("Invalid wallpaper selection"))?;
            let Some(_guard) = runtime::lock("hype-wallpaper.lock", Duration::from_secs(2))? else {
                return Ok(());
            };
            if image.is_file() {
                update_link(&current, image, |path| apply(path, true))?;
            }
        }
        path => {
            let image = Path::new(path);
            if !image.is_file() {
                return Err(err(
                    "Usage: hype-control wallpaper {ensure|init|current|restore|sync|random|select|<path>}",
                ));
            }
            update_link(&current, image, |path| apply(path, true))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    #[test]
    fn state_symlink_keeps_newlines_and_cleans_staging() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("wallpapers");
        fs::create_dir(&dir).unwrap();
        let image = dir.join("one\ntwo.PNG");
        fs::write(&image, []).unwrap();
        let link = temp.path().join("state/current_wallpaper");
        ensure(&link, &dir).unwrap();
        assert_eq!(link.canonicalize().unwrap(), image);
        assert_eq!(label(&image), format!("{}/one\\ntwo.PNG", dir.display()));
        assert_eq!(fs::read_dir(link.parent().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn unapplied_wallpaper_preserves_selection() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old.png");
        let new = temp.path().join("new.png");
        let link = temp.path().join("state/current_wallpaper");
        fs::write(&old, []).unwrap();
        fs::write(&new, []).unwrap();
        update_link(&link, &old, |_| Ok(true)).unwrap();
        assert!(update_link(&link, &new, |_| Err(err("Rejected"))).is_err());
        update_link(&link, &new, |_| Ok(false)).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), old);
        assert_eq!(fs::read_dir(link.parent().unwrap()).unwrap().count(), 1);
        update_link(&link, &new, |image| {
            assert_eq!(image, new);
            assert_eq!(fs::read_link(&link).unwrap(), old);
            Ok(true)
        })
        .unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), new);
    }
    #[test]
    fn discovery_recurses_without_following_symlink_directories() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        fs::create_dir(dir.join("nested")).unwrap();
        fs::write(dir.join("nested/1.jpg"), []).unwrap();
        fs::write(dir.join("2.webp"), []).unwrap();
        fs::write(dir.join("ignored.txt"), []).unwrap();
        symlink(dir, dir.join("nested/loop")).unwrap();
        assert_eq!(images(dir).unwrap().len(), 2);
    }
    #[test]
    fn thumbnail_cache_changes_with_source_and_limits_dimensions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.png");
        let cache = temp.path().join("cache");
        DynamicImage::new_rgb8(512, 256).save(&path).unwrap();
        let first = thumbnail(&path, &cache).unwrap();
        assert_eq!(image::image_dimensions(&first).unwrap(), (256, 128));
        assert_eq!(thumbnail(&path, &cache).unwrap(), first);
        DynamicImage::new_rgb8(32, 64).save(&path).unwrap();
        assert_ne!(thumbnail(&path, &cache).unwrap(), first);
    }
    #[test]
    fn pixel_order_matches_daemon_formats() {
        let image =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([10, 20, 30])));
        assert_eq!(
            &*pixels(&image, (1, 1), PixelFormat::Rgb).unwrap(),
            &[30, 20, 10]
        );
        assert_eq!(
            &*pixels(&image, (1, 1), PixelFormat::Bgr).unwrap(),
            &[10, 20, 30]
        );
        assert_eq!(
            &*pixels(&image, (1, 1), PixelFormat::Abgr).unwrap(),
            &[10, 20, 30, 255]
        );
    }
    #[test]
    fn outputs_share_matching_physical_size_and_format() {
        let output = |name: &str, dim, scale, pixel_format| BgInfo {
            name: name.into(),
            dim,
            scale_factor: ipc::Scale::Output(std::num::NonZeroI32::new(scale).unwrap()),
            img: ipc::BgImg::Color([0, 0, 0, 255]),
            pixel_format,
        };
        let outputs = [
            output("DP-1", (1920, 1080), 1, PixelFormat::Bgr),
            output("DP-2", (960, 540), 2, PixelFormat::Bgr),
            output("DP-3", (1920, 1080), 1, PixelFormat::Abgr),
        ];
        let groups = group_outputs(&outputs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].dim, (1920, 1080));
        assert_eq!(groups[0].names, ["DP-1", "DP-2"]);
        assert_eq!(groups[1].names, ["DP-3"]);
    }
    #[test]
    fn upstream_native_wire_roundtrip_has_center_timing_and_pixels() {
        // Socket pairs exercise the actual memfd/SCM_RIGHTS transport without any live daemon.
        let (client, server) = UnixStream::pair().unwrap();
        let client = IpcSocket::new(client.into());
        let server = IpcSocket::new(server.into());
        let mut builder = ImageRequestBuilder::new(transition(true)).unwrap();
        builder.push(
            ImgSend {
                path: "/tmp/name\nwith tab\t.png".into(),
                img: vec![3, 2, 1].into_boxed_slice(),
                dim: (1, 1),
                format: PixelFormat::Bgr,
            },
            "",
            "crop",
            None,
            "Lanczos3",
            &[],
            None,
        );
        RequestSend::Img(builder.build()).send(&client).unwrap();
        let ipc::RequestRecv::Img(request) = ipc::RequestRecv::receive(server.recv().unwrap())
        else {
            panic!("wrong request")
        };
        assert!(matches!(
            request.transition.transition_type,
            TransitionType::Grow
        ));
        assert_eq!(request.transition.duration, 0.18);
        assert_eq!(
            request.transition.bezier,
            (0.333333, 0.666667, 0.666667, 1.)
        );
        assert_eq!(request.imgs[0].dim, (1, 1));
        assert_eq!(request.imgs[0].img.bytes(), &[3, 2, 1]);
        assert_eq!(request.imgs[0].path.str(), "/tmp/name\nwith tab\t.png");
        Answer::Ok.send(&server).unwrap();
        assert!(matches!(
            Answer::receive(client.recv().unwrap()),
            Answer::Ok
        ));
    }
    #[test]
    fn query_fixture_preserves_physical_output_scale() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("awww.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let socket = IpcSocket::new(stream.into());
            assert!(matches!(
                ipc::RequestRecv::receive(socket.recv().unwrap()),
                ipc::RequestRecv::Query
            ));
            Answer::Info(
                vec![BgInfo {
                    name: "DP-1".into(),
                    dim: (1280, 800),
                    scale_factor: ipc::Scale::Fractional(std::num::NonZeroI32::new(180).unwrap()),
                    img: ipc::BgImg::Color([0, 0, 0, 255]),
                    pixel_format: PixelFormat::Bgr,
                }]
                .into_boxed_slice(),
            )
            .send(&socket)
            .unwrap();
        });
        let Answer::Info(outputs) = request_at(&path, RequestSend::Query).unwrap() else {
            panic!("wrong reply");
        };
        assert_eq!(outputs[0].real_dim(), (1920, 1200));
        server.join().unwrap();
    }
    #[test]
    fn animated_images_encode_loop_deltas() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("animated.gif");
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(fs::File::create(&path).unwrap());
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let frame = image::Frame::from_parts(
                    image::RgbaImage::from_pixel(2, 2, image::Rgba(color)),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(80, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        for format in [PixelFormat::Bgr, PixelFormat::Abgr, PixelFormat::Argb] {
            let mut budget = 200;
            let sequence = animation(&path, (4, 4), format, &mut budget)
                .unwrap()
                .unwrap();
            assert_eq!(sequence.animation.len(), 2);
            assert_eq!(sequence.animation[0].1.into_timespec().tv_nsec, 80_000_000);
            let first_frame =
                DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0])));
            let mut buffer = pixels(&first_frame, (4, 4), format).unwrap();
            awww_common::compression::Decompressor::new()
                .decompress(&sequence.animation[0].0, &mut buffer, format)
                .unwrap();
            let blue = if format.must_swap_r_and_b_channels() {
                [255, 0, 0]
            } else {
                [0, 0, 255]
            };
            assert_eq!(&buffer[..3], &blue);
            assert!(animation(&path, (4, 4), format, &mut budget).is_err());
        }
    }
}
