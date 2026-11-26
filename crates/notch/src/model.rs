#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_pause_resume_and_completion() {
        let mut timer = FocusTimer::default();
        timer.toggle(100_000_000);
        assert_eq!(timer.update(160_200_000), None);
        assert_eq!(timer.remaining, 1440);
        timer.toggle(170_000_000);
        assert_eq!(timer.remaining, 1430);
        timer.update(900_000_000);
        assert_eq!(timer.remaining, 1430);
        timer.toggle(1_000_000_000);
        assert_eq!(timer.update(2_429_999_999), None);
        assert_eq!(timer.remaining, 1);
        assert_eq!(timer.update(2_430_000_000), Some("Focus"));
        assert_eq!(timer.remaining, 300);
        assert_eq!(timer.update(99_999_000_000), None);
        assert!(timer.on_break);
        assert!(!timer.running());
    }

    #[test]
    fn skip_preserves_run_state_and_reset_restores_focus() {
        let mut timer = FocusTimer::default();
        timer.skip(0);
        assert!(timer.on_break);
        assert!(!timer.running());
        timer.toggle(0);
        timer.skip(10_000_000);
        assert_eq!(timer.remaining, 1500);
        assert!(!timer.on_break);
        assert_eq!(timer.update(1_510_000_000), Some("Focus"));
        timer.toggle(1_600_000_000);
        assert_eq!(timer.update(1_900_000_000), Some("Break"));
        timer = FocusTimer::default();
        assert_eq!(timer.remaining, 1500);
        assert!(!timer.running());
    }

    #[test]
    fn stopwatch_preserves_paused_time_and_formats_hours() {
        let mut watch = Stopwatch::default();
        watch.toggle(0);
        assert_eq!(watch.elapsed(1_500_000), 1_500_000);
        watch.toggle(2_000_000);
        assert_eq!(watch.elapsed(9_000_000), 2_000_000);
        watch.toggle(10_000_000);
        assert_eq!(watch.elapsed(10_500_000), 2_500_000);
        assert_eq!(stopwatch_text(61_250_000), "01:01.2");
        assert_eq!(stopwatch_text(3_725_000_000), "1:02:05");
    }

    #[test]
    fn weather_validates_fields_and_uses_city_only() {
        let weather =
            Weather::parse("Partly cloudy|+21°C|+20°C|40%|Testville, Region, IN\n").unwrap();
        assert_eq!(weather.temperature, "21°C");
        assert_eq!(weather.place, "Testville");
        assert_eq!(weather.detail, "Feels 20°C · 40% humidity");
        assert_eq!(weather.icon(), "weather-few-clouds-symbolic");
        for text in ["", "bad", "||||", "Sunny||+20°C|40%|City"] {
            assert!(Weather::parse(text).is_none());
        }
        assert_eq!(weather_icon("Light snow showers"), "weather-snow-symbolic");
        assert_eq!(
            weather_icon("Thundery outbreaks possible"),
            "weather-storm-symbolic"
        );
        assert_eq!(weather_icon("Smoky haze"), "weather-fog-symbolic");
        assert_eq!(weather_icon("Moderate rain"), "weather-showers-symbolic");
    }

    #[test]
    fn animation_has_original_curve_and_even_widths() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!((ease(0.245) - 0.77).abs() < 0.001);
        let mut last = 0.0;
        for i in 1..100 {
            let value = ease(f64::from(i) / 100.0);
            assert!(value >= last && value < 1.0);
            last = value;
        }
    }
}

#[derive(Debug, Clone)]
pub struct FocusTimer {
    pub on_break: bool,
    pub remaining: u32,
    deadline: Option<i64>,
}

impl Default for FocusTimer {
    fn default() -> Self {
        Self {
            on_break: false,
            remaining: 1500,
            deadline: None,
        }
    }
}

impl FocusTimer {
    pub fn duration(&self) -> u32 {
        if self.on_break { 300 } else { 1500 }
    }
    pub fn phase(&self) -> &'static str {
        if self.on_break { "Break" } else { "Focus" }
    }
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }
    pub fn update(&mut self, now: i64) -> Option<&'static str> {
        let deadline = self.deadline?;
        self.remaining = ((deadline - now).max(0) + 999_999).div_euclid(1_000_000) as u32;
        if self.remaining > 0 {
            return None;
        }
        let completed = self.phase();
        self.on_break = !self.on_break;
        self.remaining = self.duration();
        self.deadline = None;
        Some(completed)
    }
    pub fn toggle(&mut self, now: i64) -> Option<&'static str> {
        let completed = self.update(now);
        self.deadline = if self.running() {
            None
        } else {
            Some(now + i64::from(self.remaining) * 1_000_000)
        };
        completed
    }
    pub fn skip(&mut self, now: i64) {
        self.on_break = !self.on_break;
        self.remaining = self.duration();
        if self.running() {
            self.deadline = Some(now + i64::from(self.remaining) * 1_000_000);
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Stopwatch {
    pub running: bool,
    started: i64,
    accumulated: i64,
}
impl Stopwatch {
    pub fn elapsed(&self, now: i64) -> i64 {
        self.accumulated + if self.running { now - self.started } else { 0 }
    }
    pub fn toggle(&mut self, now: i64) {
        self.accumulated = self.elapsed(now);
        self.started = now;
        self.running = !self.running;
    }
}
pub fn stopwatch_text(elapsed: i64) -> String {
    let tenths = elapsed / 100_000;
    let seconds = tenths / 10;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}.{}", seconds / 60, seconds % 60, tenths % 10)
    }
}

#[derive(Debug, Clone)]
pub struct Weather {
    pub condition: String,
    pub temperature: String,
    pub detail: String,
    pub place: String,
}
impl Weather {
    pub fn parse(text: &str) -> Option<Self> {
        let fields: Vec<_> = text.trim().splitn(5, '|').collect();
        if fields.len() != 5 || fields[0].is_empty() || fields[1].is_empty() {
            return None;
        }
        Some(Self {
            condition: fields[0].into(),
            temperature: fields[1].trim_start_matches('+').into(),
            detail: format!(
                "Feels {} · {} humidity",
                fields[2].trim_start_matches('+'),
                fields[3]
            ),
            // City only; region and country crowd out the detail line.
            place: fields[4].split(',').next().unwrap_or_default().into(),
        })
    }
    pub fn icon(&self) -> &'static str {
        weather_icon(&self.condition)
    }
}
pub fn weather_icon(condition: &str) -> &'static str {
    let lower = condition.to_ascii_lowercase();
    for (words, icon) in [
        (&["thunder"][..], "weather-storm-symbolic"),
        (
            &["snow", "sleet", "ice", "blizzard"][..],
            "weather-snow-symbolic",
        ),
        (
            &["shower", "drizzle"][..],
            "weather-showers-scattered-symbolic",
        ),
        (&["rain"][..], "weather-showers-symbolic"),
        (
            &["fog", "mist", "haze", "smoke", "smoky"][..],
            "weather-fog-symbolic",
        ),
        (&["partly"][..], "weather-few-clouds-symbolic"),
        (&["cloud", "overcast"][..], "weather-overcast-symbolic"),
        (&["clear", "sunny"][..], "weather-clear-symbolic"),
    ] {
        if words.iter().any(|word| lower.contains(word)) {
            return icon;
        }
    }
    "weather-overcast-symbolic"
}

// cubic-bezier(0.32, 0.72, 0, 1): gentle start, long soft landing.
pub fn ease(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let (mut lo, mut hi, mut u) = (0.0, 1.0, t);
    for _ in 0..24 {
        u = (lo + hi) / 2.0;
        let x = 3.0 * (1.0 - u) * (1.0 - u) * u * 0.32 + u * u * u;
        if x < t {
            lo = u;
        } else {
            hi = u;
        }
    }
    3.0 * (1.0 - u).powi(2) * u * 0.72 + 3.0 * (1.0 - u) * u * u + u * u * u
}

#[cfg(test)]
mod write_queue_tests {
    use super::*;
    #[test]
    fn writes_serialize_and_keep_only_latest_pending_value() {
        let mut queue = WriteQueue::default();
        assert_eq!(queue.request(10), Some(10));
        let read_revision = queue.revision();
        assert_eq!(queue.request(20), None);
        assert_eq!(queue.request(85), None);
        assert!(queue.busy());
        assert_eq!(queue.finish(), Some(85));
        assert_eq!(queue.finish(), None);
        assert!(!queue.busy());
        assert_ne!(queue.revision(), read_revision);
    }
}

pub struct WriteQueue<T> {
    busy: bool,
    pending: Option<T>,
    revision: u64,
}
impl<T> Default for WriteQueue<T> {
    fn default() -> Self {
        Self {
            busy: false,
            pending: None,
            revision: 0,
        }
    }
}
impl<T> WriteQueue<T> {
    pub fn request(&mut self, value: T) -> Option<T> {
        self.revision = self.revision.wrapping_add(1);
        self.pending = Some(value);
        if self.busy { None } else { self.next() }
    }
    pub fn finish(&mut self) -> Option<T> {
        self.busy = false;
        self.next()
    }
    fn next(&mut self) -> Option<T> {
        let value = self.pending.take()?;
        self.busy = true;
        Some(value)
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
}
