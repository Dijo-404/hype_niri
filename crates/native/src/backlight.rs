use gtk::{gio, prelude::*};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Backlight {
    pub name: String,
    pub path: PathBuf,
    pub current: u32,
    pub maximum: u32,
}

pub fn find(root: &Path) -> Option<Backlight> {
    fs::read_dir(root)
        .ok()?
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
            (maximum > 0 && current <= maximum).then(|| Backlight {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
                current,
                maximum,
            })
        })
        .max_by_key(|light| light.maximum)
}

pub async fn write(light: &Backlight, value: u32) -> Result<(), gtk::glib::Error> {
    if fs::write(light.path.join("brightness"), value.to_string()).is_ok() {
        return Ok(());
    }
    gio::bus_get_future(gio::BusType::System)
        .await?
        .call_future(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
            "SetBrightness",
            Some(&("backlight", light.name.as_str(), value).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            3000,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn find_ignores_zero_broken_and_out_of_range_devices() {
        let root = tempfile::tempdir().unwrap();
        for (name, maximum, current) in [
            ("zero", "0", "0"),
            ("broken", "x", "3"),
            ("panel", "400", "208"),
            ("invalid", "1000", "2000"),
        ] {
            let dir = root.path().join(name);
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("max_brightness"), maximum).unwrap();
            fs::write(dir.join("brightness"), current).unwrap();
        }
        let light = find(root.path()).unwrap();
        assert_eq!((light.name.as_str(), light.current), ("panel", 208));
    }
}
