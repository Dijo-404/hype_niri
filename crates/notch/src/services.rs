use crate::pulse::Pulse;
use gtk::{gio, glib, prelude::*};
use libpulse_binding::volume::Volume;
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq)]
pub struct Backlight {
    pub light: crate::backlight::Backlight,
    pub percent: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn backlight_ignores_missing_zero_and_malformed_devices() {
        let root = std::env::temp_dir().join(format!("hype-backlight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (name, maximum, current) in [
            ("bad", "0", "10"),
            ("panel", "400", "208"),
            ("broken", "x", "3"),
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("max_brightness"), maximum).unwrap();
            fs::write(root.join(name).join("brightness"), current).unwrap();
        }
        let light = read_backlight(&root).unwrap();
        assert_eq!(light.light.name, "panel");
        assert_eq!(light.percent, 52.0);
        assert_eq!(brightness_value(0.0, 400), 4);
        assert_eq!(brightness_value(52.0, 400), 208);
        assert_eq!(brightness_value(200.0, 400), 400);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn niri_reply_uses_output_name_and_rejects_error() {
        assert_eq!(
            output_name(r#"{"Ok":{"FocusedOutput":{"name":"eDP-1","logical":{"x":0,"y":0}}}}"#)
                .unwrap(),
            Some("eDP-1".into())
        );
        assert_eq!(
            output_name(r#"{"Ok":{"FocusedOutput":null}}"#).unwrap(),
            None
        );
        assert!(output_name(r#"{"Err":"unavailable"}"#).is_err());
        assert!(output_name("garbage").is_err());
    }

    #[test]
    fn audio_worker_disconnect_resolves_request_and_clears_sender() {
        let (sender, requests) = mpsc::channel();
        drop(requests);
        let audio = Audio(Rc::new(RefCell::new(Some(sender))));
        assert_eq!(
            glib::MainContext::new().block_on(audio.volume(None)),
            Err("Audio worker stopped".into())
        );
        assert!(audio.0.borrow().is_none());
    }
}

pub fn brightness_value(percent: f64, maximum: u32) -> u32 {
    ((percent.clamp(1.0, 100.0) / 100.0 * f64::from(maximum)).round() as u32)
        .clamp(1, maximum.max(1))
}
pub fn read_backlight(root: &Path) -> Result<Backlight> {
    let light = crate::backlight::find(root).ok_or("No backlight device")?;
    let percent = (f64::from(light.current) * 100.0 / f64::from(light.maximum)).clamp(1.0, 100.0);
    Ok(Backlight { light, percent })
}
pub async fn set_brightness(device: &Backlight, percent: f64) -> Result<()> {
    let value = brightness_value(percent, device.light.maximum);
    crate::backlight::write(&device.light, value)
        .await
        .map_err(|e| e.to_string())
}
pub fn output_name(reply: &str) -> Result<Option<String>> {
    let data: serde_json::Value = serde_json::from_str(reply).map_err(|e| e.to_string())?;
    if let Some(error) = data.get("Err") {
        return Err(error.to_string());
    }
    let output = data
        .get("Ok")
        .and_then(|v| v.get("FocusedOutput"))
        .ok_or("Invalid Niri reply")?;
    if output.is_null() {
        return Ok(None);
    }
    output
        .get("name")
        .and_then(|v| v.as_str())
        .map(|name| Some(name.into()))
        .ok_or_else(|| "Missing output name".into())
}
pub fn focused_output() -> Result<Option<String>> {
    let socket = std::env::var_os("NIRI_SOCKET").ok_or("NIRI_SOCKET is unset")?;
    let mut socket = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    socket
        .write_all(b"\"FocusedOutput\"\n")
        .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(socket)
        .take(65536)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    output_name(&reply)
}
const WEATHER_URL: &str = "https://wttr.in/?format=%C|%t|%f|%h|%l";

pub async fn weather() -> Result<crate::model::Weather> {
    let process = gio::Subprocess::newv(
        &["curl", "-fsS", "--max-time", "15", WEATHER_URL].map(std::ffi::OsStr::new),
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE,
    )
    .map_err(|e| e.to_string())?;
    let (stdout, stderr) = process
        .communicate_utf8_future(None)
        .await
        .map_err(|e| e.to_string())?;
    if !process.is_successful() {
        return Err(stderr.map_or_else(|| "Weather request failed".into(), |e| e.trim().into()));
    }
    let text = stdout.unwrap_or_default();
    let text = &text[..text.floor_char_boundary(16384)];
    crate::model::Weather::parse(text).ok_or_else(|| "Invalid weather response".into())
}

const AUDIO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
pub struct Audio(Rc<RefCell<Option<mpsc::Sender<VolumeRequest>>>>);

struct VolumeRequest {
    set: Option<f64>,
    reply: Option<gio::GioFutureResult<Result<f64>>>,
}

impl VolumeRequest {
    fn resolve(&mut self, result: Result<f64>) {
        if let Some(reply) = self.reply.take() {
            reply.resolve(result);
        }
    }
}

impl Drop for VolumeRequest {
    fn drop(&mut self) {
        self.resolve(Err("Audio worker stopped".into()));
    }
}

impl Audio {
    pub async fn volume(&self, set: Option<f64>) -> Result<f64> {
        gio::GioFuture::new(self, move |audio, _, reply| {
            let mut request = VolumeRequest {
                set,
                reply: Some(reply),
            };
            let mut worker = audio.0.borrow_mut();
            if worker.is_none() {
                let (sender, requests) = mpsc::channel();
                if let Err(error) = std::thread::Builder::new()
                    .name("notch-audio".into())
                    .spawn(move || serve(requests))
                {
                    request.resolve(Err(format!("Could not start audio worker: {error}")));
                    return;
                }
                *worker = Some(sender);
            }
            if worker.as_ref().unwrap().send(request).is_err() {
                *worker = None;
            }
        })
        .await
    }
}

fn serve(requests: mpsc::Receiver<VolumeRequest>) {
    let mut connection: Option<Pulse> = None;
    for mut request in requests {
        let deadline = Instant::now() + AUDIO_TIMEOUT;
        // Process any disconnect that arrived while the worker was idle.
        if connection.as_mut().is_some_and(|pulse| !pulse.alive()) {
            connection = None;
        }
        let pulse = match connection.as_mut() {
            Some(pulse) => {
                pulse.deadline = deadline;
                pulse
            }
            None => match Pulse::connect("Niri Notch", deadline) {
                Ok(pulse) => connection.insert(pulse),
                Err(error) => {
                    request.resolve(Err(error.to_string()));
                    continue;
                }
            },
        };
        let result = volume(pulse, request.set).map_err(|e| e.to_string());
        if result.is_err() {
            connection = None;
        }
        request.resolve(result);
    }
}

fn volume(pulse: &mut Pulse, set: Option<f64>) -> std::io::Result<f64> {
    let mut sink = pulse.read(false)?;
    let Some(value) = set else {
        let percent = f64::from(sink.channels.max().0) / f64::from(Volume::NORMAL.0) * 100.0;
        return Ok(percent.clamp(0.0, 100.0));
    };
    let percent = value.clamp(0.0, 100.0);
    sink.channels.scale(Volume(
        (percent / 100.0 * f64::from(Volume::NORMAL.0)).round() as u32,
    ));
    pulse.set_sink_volume(sink.index, &sink.channels)?;
    Ok(percent)
}

#[derive(Clone, Debug, Default)]
pub struct Media {
    pub name: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
}
impl Media {
    pub fn from_proxy(proxy: &gio::DBusProxy) -> Self {
        let boolean = |key| {
            proxy
                .cached_property(key)
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false)
        };
        let metadata = proxy
            .cached_property("Metadata")
            .and_then(|v| v.get::<std::collections::HashMap<String, glib::Variant>>())
            .unwrap_or_default();
        let name = proxy
            .name()
            .unwrap_or_default()
            .trim_start_matches("org.mpris.MediaPlayer2.")
            .to_string();
        let title = metadata
            .get("xesam:title")
            .and_then(|v| v.str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&name)
            .to_string();
        let artist = metadata
            .get("xesam:artist")
            .and_then(|v| v.get::<Vec<String>>())
            .unwrap_or_default()
            .join(", ");
        Self {
            name,
            title,
            artist,
            playing: proxy
                .cached_property("PlaybackStatus")
                .and_then(|v| v.str().map(|s| s == "Playing"))
                .unwrap_or(false),
            can_play: boolean("CanPlay"),
            can_pause: boolean("CanPause"),
            can_previous: boolean("CanGoPrevious"),
            can_next: boolean("CanGoNext"),
        }
    }
    pub fn active(&self) -> bool {
        self.playing || self.can_play || self.can_pause
    }
}
