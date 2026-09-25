//! Settings file (/etc/gpu-fan-controller.conf on Linux,
//! %ProgramData%\gpu-fanctl\gpu-fan-controller.conf on Windows):
//!
//! ```text
//! max_temp = 80                          # fans go to 100 % at this GPU temperature
//! curve = 40:30, 55:40, 65:60, 80:100    # a custom curve, or `auto` (the default)
//! custom_curve = 40:30, 55:40, 80:100    # with `curve = auto`: the custom curve, remembered
//! ```

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::curve::{CUSTOM_POINTS, CUSTOM_TEMPS, Curve, FLOOR_DUTY};

pub const DEFAULT_MAX_TEMP: u32 = 80;
pub const MAX_TEMP_RANGE: RangeInclusive<u32> = 60..=90;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub max_temp: u32,
    /// The curve in use.
    pub curve: Curve,
    /// While `curve` is Auto: the user's custom curve, kept so switching back restores it.
    /// Always `None` while a custom curve is in use (it is then `curve` itself).
    pub remembered_custom: Option<Vec<(f32, f32)>>,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_temp: DEFAULT_MAX_TEMP, curve: Curve::Auto, remembered_custom: None }
    }
}

impl Config {
    /// The user's custom points: the active custom curve, or the one remembered while Auto runs.
    pub fn custom_points(&self) -> Option<&[(f32, f32)]> {
        match &self.curve {
            Curve::Custom(points) => Some(points),
            Curve::Auto => self.remembered_custom.as_deref(),
        }
    }

    /// Switches to the Auto curve, remembering the custom curve.
    pub fn use_auto(&mut self) {
        self.remembered_custom = self.custom_points().map(<[_]>::to_vec);
        self.curve = Curve::Auto;
    }

    /// Switches back to the remembered custom curve.
    pub fn use_remembered_custom(&mut self) -> Result<(), String> {
        let points = self.custom_points().ok_or("no custom curve has been set yet")?.to_vec();
        self.set_custom(Curve::custom(points)?);
        Ok(())
    }

    /// Uses `curve` (a `Curve::Custom`) as the custom curve.
    pub fn set_custom(&mut self, curve: Curve) {
        self.curve = curve;
        self.remembered_custom = None;
    }
}

/// Directory for the config file and, on Windows, the service log.
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        let program_data = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
        Path::new(&program_data).join("gpu-fanctl")
    } else {
        PathBuf::from("/etc")
    }
}

/// The settings file. The `GPU_FANCTL_CONFIG` environment variable overrides it (for testing).
pub fn path() -> PathBuf {
    std::env::var_os("GPU_FANCTL_CONFIG").map_or_else(|| data_dir().join("gpu-fan-controller.conf"), PathBuf::from)
}

/// Parses the config text. Missing keys keep their defaults.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut config = Config::default();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = value`", number + 1));
        };
        let value = value.trim();
        match key.trim() {
            "max_temp" => config.max_temp = validate_max_temp(value)?,
            "curve" => config.curve = parse_curve(value)?,
            "custom_curve" => match parse_curve(value)? {
                Curve::Custom(points) => config.remembered_custom = Some(points),
                Curve::Auto => return Err(format!("line {}: custom_curve needs points, not `auto`", number + 1)),
            },
            other => return Err(format!("line {}: unknown key `{other}`", number + 1)),
        }
    }
    if matches!(config.curve, Curve::Custom(_)) {
        config.remembered_custom = None;
    }
    Ok(config)
}

/// Parses and range-checks a max temp value given as text.
pub fn validate_max_temp(value: &str) -> Result<u32, String> {
    let temp: u32 = value.parse().map_err(|_| format!("max_temp `{value}` is not a whole number"))?;
    if MAX_TEMP_RANGE.contains(&temp) {
        Ok(temp)
    } else {
        Err(format!(
            "max_temp {temp} is outside {}..={} °C",
            MAX_TEMP_RANGE.start(),
            MAX_TEMP_RANGE.end()
        ))
    }
}

/// Parses a curve: `auto`, or `temp:duty` points separated by commas and/or spaces
/// (e.g. `40:30, 55:40, 80:100`). Custom points are validated by `Curve::custom`.
pub fn parse_curve(value: &str) -> Result<Curve, String> {
    if value.trim().eq_ignore_ascii_case("auto") {
        return Ok(Curve::Auto);
    }
    let points = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|point| {
            let (temp, duty) = point
                .split_once(':')
                .ok_or_else(|| format!("curve point `{point}` is not `temperature:duty`"))?;
            let number = |s: &str| {
                s.trim_end_matches(['%', 'C', '°']).parse::<u32>().map(|n| n as f32).map_err(|_| {
                    format!("curve point `{point}`: `{s}` is not a whole number")
                })
            };
            Ok((number(temp)?, number(duty)?))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Curve::custom(points).map_err(|e| format!("curve: {e}"))
}

pub fn format_points(points: &[(f32, f32)]) -> String {
    points.iter().map(|(t, d)| format!("{t}:{d}")).collect::<Vec<_>>().join(", ")
}

pub fn format_curve(curve: &Curve) -> String {
    match curve {
        Curve::Auto => "auto".to_string(),
        Curve::Custom(points) => format_points(points),
    }
}

pub fn render(config: &Config) -> String {
    let mut curve_lines = format!("curve = {}", format_curve(&config.curve));
    if let (Curve::Auto, Some(points)) = (&config.curve, &config.remembered_custom) {
        curve_lines.push_str(&format!("\ncustom_curve = {}", format_points(points)));
    }
    format!(
        "# gpu-fanctl settings\n\
         #\n\
         # max_temp: GPU temperature (°C) at which the external fans always run at 100 %.\n\
         #   Allowed {}..={}, default {}.\n\
         max_temp = {}\n\
         #\n\
         # curve: `auto` for the built-in Auto curve (30 % up to 45 °C, rising to 100 % at max_temp),\n\
         #   or a custom fan curve as temperature:duty points (°C:%), rising left to right:\n\
         #   {}..={} points, temperatures {}..={} °C, duty {}..=100 %.\n\
         # custom_curve: with `curve = auto`, your custom curve is remembered here.\n\
         {curve_lines}\n",
        MAX_TEMP_RANGE.start(),
        MAX_TEMP_RANGE.end(),
        DEFAULT_MAX_TEMP,
        config.max_temp,
        CUSTOM_POINTS.start(),
        CUSTOM_POINTS.end(),
        CUSTOM_TEMPS.start(),
        CUSTOM_TEMPS.end(),
        FLOOR_DUTY,
    )
}

/// Loads the config. A missing file gives the defaults; an invalid one gives the defaults
/// (Auto curve, max temp 80) plus a warning to log.
pub fn load(path: &Path) -> (Config, Option<String>) {
    let fallback = |e: String| {
        let warning = format!("{}: {e}; using the Auto curve and max temp {DEFAULT_MAX_TEMP} °C", path.display());
        (Config::default(), Some(warning))
    };
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map_or_else(fallback, |c| (c, None)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Config::default(), None),
        Err(e) => fallback(e.to_string()),
    }
}

pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, render(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points() -> Vec<(f32, f32)> {
        vec![(40.0, 30.0), (60.0, 50.0), (80.0, 100.0)]
    }

    #[test]
    fn empty_config_gives_defaults() {
        assert_eq!(parse(""), Ok(Config::default()));
        assert_eq!(parse("# only a comment\n\n"), Ok(Config::default()));
    }

    #[test]
    fn parses_max_temp_with_comments_and_spacing() {
        assert_eq!(parse("max_temp=75").unwrap().max_temp, 75);
        assert_eq!(parse("  max_temp =  82  # quieter\n").unwrap().max_temp, 82);
    }

    #[test]
    fn rejects_bad_max_temp_and_garbage() {
        assert!(parse("max_temp = 59").is_err());
        assert!(parse("max_temp = 91").is_err());
        assert!(parse("max_temp = hot").is_err());
        assert!(parse("max_temp = 80.5").is_err());
        assert!(parse("fan = 3").is_err());
        assert!(parse("max_temp 80").is_err());
    }

    #[test]
    fn parses_custom_curve_in_several_spellings() {
        let expected = Curve::Custom(points());
        for text in ["40:30, 60:50, 80:100", "40:30 60:50 80:100", "40C:30%,60°C:50%, 80:100"] {
            assert_eq!(parse_curve(text), Ok(expected.clone()), "{text}");
        }
        assert_eq!(parse_curve("Auto"), Ok(Curve::Auto));
        let config = parse("max_temp = 85\ncurve = 40:30, 60:50, 80:100\n").unwrap();
        assert_eq!(config, Config { max_temp: 85, curve: expected, remembered_custom: None });
    }

    #[test]
    fn rejects_bad_curves() {
        assert!(parse_curve("40:30").is_err(), "one point");
        assert!(parse_curve("40-30, 60:50").is_err(), "bad separator");
        assert!(parse_curve("40:30, 60:abc").is_err(), "not a number");
        assert!(parse_curve("60:30, 40:50").is_err(), "temps falling");
        assert!(parse_curve("40:10, 60:50").is_err(), "below floor");
        assert!(parse("curve = 40:30, 30:50").is_err());
        assert!(parse("custom_curve = auto").is_err());
    }

    #[test]
    fn render_round_trips() {
        for config in [
            Config { max_temp: 60, curve: Curve::Auto, remembered_custom: None },
            Config { max_temp: 90, curve: Curve::Custom(points()), remembered_custom: None },
            Config { max_temp: 75, curve: Curve::Auto, remembered_custom: Some(points()) },
            Config::default(),
        ] {
            assert_eq!(parse(&render(&config)), Ok(config));
        }
    }

    #[test]
    fn custom_curve_survives_switching_to_auto_and_back() {
        let mut config = parse("curve = 40:30, 60:50, 80:100").unwrap();
        config.use_auto();
        assert_eq!(config.curve, Curve::Auto);
        // Remembered in the file as well, so it survives a restart.
        let mut reloaded = parse(&render(&config)).unwrap();
        assert_eq!(reloaded.custom_points(), Some(points().as_slice()));
        reloaded.use_remembered_custom().unwrap();
        assert_eq!(reloaded.curve, Curve::Custom(points()));
        assert_eq!(reloaded.remembered_custom, None);
    }

    #[test]
    fn older_files_still_load() {
        // Files written before custom_curve existed.
        assert_eq!(parse("max_temp = 80\n# curve = 45:30, 60:45, 70:70, 80:100\n"), Ok(Config::default()));
        assert!(Config::default().use_remembered_custom().is_err());
    }
}
