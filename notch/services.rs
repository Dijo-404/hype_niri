use gtk::{gio, glib, prelude::*};
use libpulse_binding::{
    callbacks::ListResult,
    context::{Context, FlagSet, State},
    mainloop::standard::Mainloop,
    operation::{Operation, State as OperationState},
    time::MicroSeconds,
    volume::Volume,
};
use std::{
    cell::{Cell, RefCell},
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq)]
pub struct Backlight {
    pub name: String,
    pub maximum: u32,
    pub percent: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(light.name, "panel");
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
    fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let maximum = fs::read_to_string(path.join("max_brightness"))
                .ok()?
                .trim()
                .parse::<u32>()
                .ok()?;
            let current = fs::read_to_string(path.join("brightness"))
                .ok()?
                .trim()
                .parse::<u32>()
                .ok()?;
            (maximum > 0).then(|| Backlight {
                name: entry.file_name().to_string_lossy().into_owned(),
                maximum,
                percent: (f64::from(current) * 100.0 / f64::from(maximum)).clamp(1.0, 100.0),
            })
        })
        .max_by_key(|device| device.maximum)
        .ok_or_else(|| "No backlight device".into())
}
pub async fn set_brightness(device: &Backlight, percent: f64) -> Result<()> {
    let value = brightness_value(percent, device.maximum);
    let path = PathBuf::from("/sys/class/backlight")
        .join(&device.name)
        .join("brightness");
    if fs::write(path, value.to_string()).is_ok() {
        return Ok(());
    }
    let bus = gio::bus_get_future(gio::BusType::System)
        .await
        .map_err(|e| e.to_string())?;
    bus.call_future(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
        "SetBrightness",
        Some(&("backlight", device.name.as_str(), value).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        3000,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
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
pub fn weather() -> Result<crate::model::Weather> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get("https://wttr.in/?format=%C|%t|%f|%h|%l")
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let mut text = String::new();
    response
        .take(16384)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    crate::model::Weather::parse(&text).ok_or_else(|| "Invalid weather response".into())
}

const AUDIO_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_SINK: &str = "@DEFAULT_SINK@";

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
                    .spawn(move || PulseAudio::serve(requests))
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

struct PulseAudio {
    context: Context,
    mainloop: Mainloop,
}

impl PulseAudio {
    fn serve(requests: mpsc::Receiver<VolumeRequest>) {
        let mut connection = None;
        for mut request in requests {
            let deadline = Instant::now() + AUDIO_TIMEOUT;
            if connection.as_mut().is_some_and(|audio: &mut Self| {
                // Process any disconnect that arrived while the worker was idle.
                !audio.mainloop.iterate(false).is_success()
                    || audio.context.get_state() != State::Ready
            }) {
                connection = None;
            }
            if connection.is_none() {
                match Self::connect(deadline) {
                    Ok(audio) => connection = Some(audio),
                    Err(error) => {
                        request.resolve(Err(error));
                        continue;
                    }
                }
            }
            let result = connection.as_mut().unwrap().volume(request.set, deadline);
            if result.is_err() {
                connection = None;
            }
            request.resolve(result);
        }
    }

    fn connect(deadline: Instant) -> Result<Self> {
        let mainloop = Mainloop::new().ok_or("Audio mainloop unavailable")?;
        let context = Context::new(&mainloop, "Niri Notch").ok_or("Audio context unavailable")?;
        let mut audio = Self { context, mainloop };
        audio
            .context
            .connect(None, FlagSet::NOAUTOSPAWN, None)
            .map_err(Self::connection_error)?;
        while audio.context.get_state() != State::Ready {
            audio.pump(deadline)?;
        }
        Ok(audio)
    }

    fn connection_error(error: libpulse_binding::error::PAErr) -> String {
        error
            .to_string()
            .unwrap_or_else(|| "Audio connection failed".into())
    }

    fn pump(&mut self, deadline: Instant) -> Result<()> {
        if matches!(self.context.get_state(), State::Failed | State::Terminated) {
            return Err("Audio service unavailable".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Audio request timed out".into());
        }
        self.mainloop
            .prepare(Some(MicroSeconds(remaining.as_micros() as u64)))
            .and_then(|()| self.mainloop.poll())
            .and_then(|_| self.mainloop.dispatch())
            .map_err(Self::connection_error)?;
        Ok(())
    }

    fn wait<T: ?Sized>(&mut self, mut operation: Operation<T>, deadline: Instant) -> Result<()> {
        while operation.get_state() == OperationState::Running {
            if let Err(error) = self.pump(deadline) {
                operation.cancel();
                return Err(error);
            }
        }
        if operation.get_state() != OperationState::Done {
            return Err("Audio request cancelled".into());
        }
        Ok(())
    }

    fn volume(&mut self, set: Option<f64>, deadline: Instant) -> Result<f64> {
        let channels = Rc::new(Cell::new(None));
        let result = channels.clone();
        let operation =
            self.context
                .introspect()
                .get_sink_info_by_name(DEFAULT_SINK, move |info| {
                    if let ListResult::Item(info) = info {
                        result.set(Some(info.volume));
                    }
                });
        self.wait(operation, deadline)?;
        let mut channels = channels.get().ok_or("Default audio sink unavailable")?;
        let Some(value) = set else {
            let percent = f64::from(channels.max().0) / f64::from(Volume::NORMAL.0) * 100.0;
            return Ok(percent.clamp(0.0, 100.0));
        };
        let percent = value.clamp(0.0, 100.0);
        channels.scale(Volume(
            (percent / 100.0 * f64::from(Volume::NORMAL.0)).round() as u32,
        ));
        let success = Rc::new(Cell::new(false));
        let result = success.clone();
        let operation = self.context.introspect().set_sink_volume_by_name(
            DEFAULT_SINK,
            &channels,
            Some(Box::new(move |ok| result.set(ok))),
        );
        self.wait(operation, deadline)?;
        if !success.get() {
            return Err("Could not change volume".into());
        }
        Ok(percent)
    }
}

impl Drop for PulseAudio {
    fn drop(&mut self) {
        if self.context.get_state().is_good() {
            self.context.disconnect();
        }
    }
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
