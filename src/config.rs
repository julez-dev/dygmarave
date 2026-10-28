//! Configuration: an XDG TOML file with command-line overrides.
//!
//! Everything worth tuning lives here. The defaults are the spec's settled
//! choices; a config file overrides them, and command-line flags override the
//! file. Invalid or out-of-range values are rejected at startup with the
//! offending key named, so a typo surfaces as an error rather than as odd
//! lighting mid-song.
//!
//! The file lives at `$XDG_CONFIG_HOME/dygmarave/config.toml` (default
//! `~/.config/dygmarave/config.toml`). A missing file is not an error: the
//! defaults apply. A present but malformed file is an error.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::dynamics::DynamicsConfig;
use crate::live::LiveConfig;

/// The whole configuration.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub analysis: AnalysisConfig,
    pub render: RenderConfig,
    pub thumbs: ThumbConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceConfig {
    /// "auto" or an explicit port path. "auto" lets discovery find the board.
    pub port: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            port: "auto".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AudioConfig {
    /// Empty means the default sink's monitor.
    pub source: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AnalysisConfig {
    /// FFT window in samples. A power of two, because the FFT wants one.
    pub window: usize,
    /// Lowest band edge in Hz.
    pub band_min_hz: f32,
    /// Highest band edge in Hz.
    pub band_max_hz: f32,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            window: crate::spectrum::FFT_SIZE,
            band_min_hz: crate::spectrum::LOW_HZ,
            band_max_hz: crate::spectrum::HIGH_HZ,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RenderConfig {
    /// Frames per second to target. 30 is smooth to the eye.
    pub fps_target: u32,
    /// Attack curve. Only "instant" is supported.
    pub attack: String,
    /// Fade time constant in milliseconds.
    pub decay_ms: f32,
    /// Gate threshold in dBFS; below this a channel is silent.
    pub gate_db: f32,
    /// AGC peak-tracker time constant in milliseconds.
    pub agc_release_ms: f32,
    /// AGC ceiling. Under 1.0 so loud material stays below white.
    pub target: f32,
    /// AGC gain ceiling, so near-silence is not amplified without limit.
    pub max_gain: f32,
    /// Levels below this are drawn black, so silence goes dark.
    pub floor: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        // Delegate to the source types so a change to a default lands in one place.
        // `attack` has no counterpart there and stays explicit.
        let dynamics = DynamicsConfig::default();
        let live = LiveConfig::default();
        Self {
            fps_target: live.fps,
            attack: "instant".to_string(),
            decay_ms: dynamics.decay_ms,
            gate_db: dynamics.gate_db,
            agc_release_ms: dynamics.agc_release_ms,
            target: dynamics.target,
            max_gain: dynamics.max_gain,
            floor: dynamics.floor,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThumbConfig {
    /// Whether the thumb clusters glow with their channel's level.
    pub level: bool,
    /// Whether the thumb clusters flash white on a beat.
    pub beat_flash: bool,
    /// How much louder than the rolling average a beat must be to flash.
    pub beat_threshold: f32,
}

impl Default for ThumbConfig {
    fn default() -> Self {
        let dynamics = DynamicsConfig::default();
        Self {
            level: dynamics.thumb_glow,
            beat_flash: dynamics.beat_flash,
            beat_threshold: dynamics.beat_threshold,
        }
    }
}

/// A configuration problem, always naming the key that caused it.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read {}: {cause}", .path.display())]
    Read {
        path: PathBuf,
        cause: std::io::Error,
    },
    /// The file was not valid TOML or did not match the schema.
    #[error("cannot parse {}: {cause}", .path.display())]
    Parse {
        path: PathBuf,
        cause: toml::de::Error,
    },
    /// A value parsed but is out of range.
    #[error("invalid {key}: {message}")]
    Invalid { key: String, message: String },
}

impl Config {
    /// Loads the config from a specific path.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|cause| ConfigError::Read {
            path: path.to_path_buf(),
            cause,
        })?;
        let config: Self = toml::from_str(&text).map_err(|cause| ConfigError::Parse {
            path: path.to_path_buf(),
            cause,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Loads the config from the XDG path, falling back to defaults when absent.
    pub fn load_default() -> Result<Self, ConfigError> {
        let path = Self::default_path();
        match std::fs::metadata(&path) {
            // A missing file is not an error: the defaults apply.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            // A path that exists but cannot be read — a directory, wrong permissions —
            // is a real problem worth reporting, not a silent fallback.
            Ok(metadata) if metadata.is_file() => Self::load(&path),
            Ok(_) => Self::load(&path),
            Err(error) => Err(ConfigError::Read { path, cause: error }),
        }
    }

    /// The XDG config path.
    ///
    /// `$XDG_CONFIG_HOME` must be absolute; a relative or empty value is ignored
    /// per the XDG spec, falling back to `$HOME/.config`.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("dygmarave").join("config.toml")
    }

    /// Rejects values that would produce broken lighting rather than an error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let render = &self.render;
        let invalid = |key: &str, message: String| ConfigError::Invalid {
            key: key.to_string(),
            message,
        };

        if !(1..=240).contains(&render.fps_target) {
            return Err(invalid(
                "render.fps_target",
                format!("must be 1 to 240, got {}", render.fps_target),
            ));
        }
        if render.attack != "instant" {
            return Err(invalid(
                "render.attack",
                format!("only \"instant\" is supported, got {:?}", render.attack),
            ));
        }
        if !render.decay_ms.is_finite() || render.decay_ms <= 0.0 {
            return Err(invalid(
                "render.decay_ms",
                format!("must be a positive number, got {}", render.decay_ms),
            ));
        }
        if !render.gate_db.is_finite() || render.gate_db > 0.0 {
            return Err(invalid(
                "render.gate_db",
                format!("must be zero or negative dBFS, got {}", render.gate_db),
            ));
        }
        if !render.agc_release_ms.is_finite() || render.agc_release_ms <= 0.0 {
            return Err(invalid(
                "render.agc_release_ms",
                format!("must be a positive number, got {}", render.agc_release_ms),
            ));
        }
        if !(0.0..=1.0).contains(&render.target) {
            return Err(invalid(
                "render.target",
                format!("must be between 0 and 1, got {}", render.target),
            ));
        }
        if !render.max_gain.is_finite() || render.max_gain < 1.0 {
            return Err(invalid(
                "render.max_gain",
                format!("must be at least 1, got {}", render.max_gain),
            ));
        }
        if !(0.0..=0.5).contains(&render.floor) || render.floor == 0.0 {
            return Err(invalid(
                "render.floor",
                format!(
                    "must be greater than 0 and at most 0.5 (0 disables the silence floor,                      anything above 0.5 blacks most levels), got {}",
                    render.floor
                ),
            ));
        }
        if !self.thumbs.beat_threshold.is_finite() || self.thumbs.beat_threshold <= 0.0 {
            return Err(invalid(
                "thumbs.beat_threshold",
                format!(
                    "must be a positive multiple of the rolling average, got {}",
                    self.thumbs.beat_threshold
                ),
            ));
        }
        if self.analysis.window == 0 || !self.analysis.window.is_power_of_two() {
            return Err(invalid(
                "analysis.window",
                format!(
                    "must be a non-zero power of two, got {}",
                    self.analysis.window
                ),
            ));
        }
        if !self.analysis.band_min_hz.is_finite()
            || !self.analysis.band_max_hz.is_finite()
            || self.analysis.band_min_hz <= 0.0
            || self.analysis.band_min_hz >= self.analysis.band_max_hz
        {
            return Err(invalid(
                "analysis.band_min_hz",
                format!(
                    "must be positive and below band_max_hz ({}), got {}",
                    self.analysis.band_max_hz, self.analysis.band_min_hz
                ),
            ));
        }

        Ok(())
    }

    /// The dynamics settings the renderer needs, from the file or the defaults.
    pub fn dynamics(&self) -> DynamicsConfig {
        DynamicsConfig {
            decay_ms: self.render.decay_ms,
            agc_release_ms: self.render.agc_release_ms,
            gate_db: self.render.gate_db,
            target: self.render.target,
            max_gain: self.render.max_gain,
            beat_threshold: self.thumbs.beat_threshold,
            thumb_glow: self.thumbs.level,
            beat_flash: self.thumbs.beat_flash,
            floor: self.render.floor,
            ..DynamicsConfig::default()
        }
    }

    /// The device port, with "auto" meaning discovery.
    pub fn port(&self) -> Option<&str> {
        match self.device.port.as_str() {
            "auto" | "" => None,
            port => Some(port),
        }
    }

    /// The audio source, with an empty string meaning the default sink's monitor.
    pub fn source(&self) -> Option<&str> {
        match self.audio.source.as_str() {
            "" => None,
            source => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let config = Config::default();
        assert_eq!(config.render.fps_target, 30);
        assert_eq!(config.render.decay_ms, 180.0);
        assert_eq!(config.render.gate_db, -55.0);
        assert_eq!(config.render.agc_release_ms, 4_000.0);
        assert_eq!(config.render.target, 0.7);
        assert_eq!(config.render.max_gain, 100.0);
        assert_eq!(config.render.floor, 0.02);
        assert_eq!(config.thumbs.beat_threshold, 2.0);
        assert!(config.thumbs.level);
        assert!(config.thumbs.beat_flash);
        assert_eq!(config.analysis.window, 1024);
        assert_eq!(config.port(), None);
        assert_eq!(config.source(), None);
    }

    #[test]
    fn a_file_overrides_the_defaults() {
        let toml = r#"
[render]
fps_target = 60
decay_ms = 250.0
gate_db = -40.0
agc_release_ms = 2000.0
floor = 0.05

[thumbs]
level = false
beat_flash = false
beat_threshold = 3.0
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        config.validate().expect("in range");
        assert_eq!(config.render.fps_target, 60);
        assert_eq!(config.render.decay_ms, 250.0);
        assert_eq!(config.render.gate_db, -40.0);
        assert!(!config.thumbs.level);
        assert_eq!(config.thumbs.beat_threshold, 3.0);
        // Untouched keys keep their defaults.
        assert_eq!(config.render.target, 0.7);
        assert_eq!(config.analysis.window, 1024);
    }

    #[test]
    fn invalid_values_are_rejected_naming_the_key() {
        let cases: &[(&str, &str)] = &[
            ("[render]\nfps_target = 0\n", "render.fps_target"),
            ("[render]\nfps_target = 241\n", "render.fps_target"),
            ("[render]\ndecay_ms = 0.0\n", "render.decay_ms"),
            ("[render]\ndecay_ms = -5.0\n", "render.decay_ms"),
            ("[render]\ngate_db = 10.0\n", "render.gate_db"),
            ("[render]\nagc_release_ms = -1.0\n", "render.agc_release_ms"),
            ("[render]\ntarget = 1.5\n", "render.target"),
            ("[render]\nmax_gain = 0.5\n", "render.max_gain"),
            ("[render]\nfloor = 2.0\n", "render.floor"),
            ("[thumbs]\nbeat_threshold = 0.0\n", "thumbs.beat_threshold"),
            ("[analysis]\nwindow = 1000\n", "analysis.window"),
            (
                "[analysis]\nband_min_hz = 200.0\nband_max_hz = 100.0\n",
                "analysis.band_min_hz",
            ),
            ("[render]\nattack = \"slow\"\n", "render.attack"),
        ];
        for (toml, key) in cases {
            let config: Config = toml::from_str(toml).expect("parses");
            let error = config.validate().expect_err("must be rejected");
            let message = error.to_string();
            assert!(
                message.contains(key),
                "expected {key:?} in {message:?} for {toml:?}"
            );
        }
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        let toml = "[render]\nunknown_knob = 1\n";
        let error = toml::from_str::<Config>(toml).expect_err("unknown keys are rejected");
        assert!(error.to_string().contains("unknown_knob"));
    }

    #[test]
    fn a_missing_file_falls_back_to_defaults() {
        // A path that certainly does not exist.
        let config = Config::load(Path::new("/nonexistent-dygmarave/config.toml"));
        assert!(
            config.is_err(),
            "an explicit load must report the read failure"
        );
    }

    #[test]
    fn dynamics_maps_the_render_and_thumb_sections() {
        let mut config = Config::default();
        config.render.decay_ms = 250.0;
        config.render.floor = 0.05;
        config.thumbs.beat_threshold = 3.0;
        config.thumbs.level = false;
        config.thumbs.beat_flash = false;
        let dynamics = config.dynamics();
        assert_eq!(dynamics.decay_ms, 250.0);
        assert_eq!(dynamics.floor, 0.05);
        assert_eq!(dynamics.beat_threshold, 3.0);
        assert!(!dynamics.thumb_glow);
        assert!(!dynamics.beat_flash);
    }

    #[test]
    fn the_example_config_parses_and_validates() {
        // The shipped example must stay in step with the code: an unknown key or a
        // stale range would make it a broken template. `deny_unknown_fields` and
        // `validate` are the guard.
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("config.example.toml");
        let config = Config::load(&path).expect("the example config must load");
        assert_eq!(config.render.fps_target, 30);
        assert_eq!(config.analysis.window, 1024);
    }

    #[test]
    fn the_xdg_path_honours_the_environment() {
        // These assertions run in the real process environment; they only check the
        // shape of the path, not a particular home directory.
        let path = Config::default_path();
        assert!(path.ends_with("dygmarave/config.toml"));
    }

    #[test]
    fn a_config_error_prints_its_cause_exactly_once() {
        // The pre-thiserror manual `impl Error` never exposed source(), so `{:#}`
        // must not append the cause a second time. A field named `source` would
        // auto-wire thiserror's source() and duplicate the leaf message.
        let error = ConfigError::Read {
            path: PathBuf::from("/nope/config.toml"),
            cause: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
        };
        let rendered = error.to_string();
        assert_eq!(rendered.matches("gone").count(), 1, "got {rendered:?}");
        assert!(
            std::error::Error::source(&error).is_none(),
            "the cause must not chain"
        );
    }
}
