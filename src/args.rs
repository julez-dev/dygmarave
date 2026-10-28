//! Command-line arguments.
//!
//! Parsed by `clap` (derive) so the parser and the help text cannot drift apart:
//! the same attributes that accept a flag describe it. The flag surface is the
//! hand-rolled parser's, unchanged — including the aliases and the fact that
//! `--probe-spectrum` implies `--probe-audio`.

use std::ffi::OsString;

use clap::Parser;

use crate::layout::LayoutKind;
use crate::motion::Motion;
use crate::palette::Palette;
use crate::theme::Rgb;

#[derive(Debug, Parser)]
#[command(
    name = "dygmarave",
    version,
    about = "Audio-reactive per-key LEDs for the Dygma Raise",
    after_help = "With --probe-audio, the default sink's monitor is captured (never the \
microphone) and a live level meter is printed, failing if no signal arrives. \
--probe-spectrum replaces the meter with one character per frequency band. \
--live drives the board from the output mix until Ctrl-C: a moving spectrum, \
band energies through the colour ramp, instant attack and slow decay.\n\n\
--layout, --palette and --motion choose a fixed look axis or \"random\"; random \
pools and intervals come from [look] in the config file. --seed makes the random \
draws reproducible and --quiet suppresses the seed and switch log.\n\n\
The board is found by USB identity, not by a fixed port name. The LED count is \
read from the board itself. The baseline theme and LED mode are restored on exit \
and when Ctrl-C is pressed once; pressing Ctrl-C a second time aborts without \
restoring."
)]
pub struct Args {
    /// serial port to use, skipping USB discovery
    #[arg(long)]
    pub port: Option<String>,

    /// colour to paint and measure with
    #[arg(long = "colour", visible_alias = "color", default_value = "32,96,255", value_parser = parse_colour)]
    pub colour: Rgb,

    /// hold the colour this long before checking it
    #[arg(long = "hold-ms", default_value_t = 3_000)]
    pub hold_ms: u64,

    /// frames to write when measuring
    #[arg(long, default_value_t = 120)]
    pub frames: u32,

    /// probe the audio path instead of the LED path
    #[arg(long = "probe-audio", conflicts_with = "live")]
    pub probe_audio: bool,

    /// static hardware probe: paint each physical column a different colour (not a live layout)
    #[arg(long = "probe-columns", alias = "columns")]
    pub probe_columns: bool,

    /// static hardware probe: paint each physical row a different colour (not a live layout)
    #[arg(long = "probe-rows", alias = "rows")]
    pub probe_rows: bool,

    /// static hardware probe: paint the snake band sequence as a hue ramp (not a live layout)
    #[arg(long = "probe-gradient", alias = "gradient")]
    pub probe_gradient: bool,

    /// static hardware probe: light the guessed thumb clusters (not a live layout)
    #[arg(long = "probe-thumbs", alias = "thumbs", alias = "thumb-pattern")]
    pub probe_thumbs: bool,

    /// probe audio and print 28 log-spaced band energies
    #[arg(long = "probe-spectrum", conflicts_with = "live")]
    pub probe_spectrum: bool,

    /// drive the board live from whatever the speakers are playing
    #[arg(long)]
    pub live: bool,

    /// fixed layout name, or "random" (pools and intervals come from [look])
    #[arg(long, value_parser = parse_layout_choice)]
    pub layout: Option<String>,

    /// fixed palette name, or "random"
    #[arg(long, value_parser = parse_palette_choice)]
    pub palette: Option<String>,

    /// fixed motion name, or "random"
    #[arg(long, value_parser = parse_motion_choice)]
    pub motion: Option<String>,

    /// seed for random draws; omit to use OS entropy
    #[arg(long)]
    pub seed: Option<u64>,

    /// suppress the startup seed line and per-switch lines
    #[arg(long)]
    pub quiet: bool,

    /// frames per second for --live (default 30; lowered automatically when the
    /// board cannot keep up)
    #[arg(long, value_parser = parse_fps)]
    pub fps: Option<u32>,

    /// how long to probe audio for
    #[arg(long, default_value_t = 5)]
    pub seconds: u64,

    /// capture device to use instead of choosing one
    #[arg(long)]
    pub source: Option<String>,
}

/// Parses the full argument list, program name included.
///
/// `--probe-spectrum` implies `--probe-audio`: the flag only changes how the
/// audio readout is printed. The implication runs after clap's own validation, so
/// the declarative conflicts still see the flags as written.
pub fn parse_args<I, T>(args: I) -> Result<Args, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut parsed = Args::try_parse_from(args)?;
    if parsed.probe_spectrum {
        parsed.probe_audio = true;
    }
    Ok(parsed)
}

fn parse_layout_choice(value: &str) -> Result<String, String> {
    parse_axis_choice(value, "layout", |name| {
        LayoutKind::from_name(name).is_some()
    })
}

fn parse_palette_choice(value: &str) -> Result<String, String> {
    parse_axis_choice(value, "palette", |name| Palette::from_name(name).is_some())
}

fn parse_motion_choice(value: &str) -> Result<String, String> {
    parse_axis_choice(value, "motion", |name| Motion::from_name(name).is_some())
}

fn parse_axis_choice(
    value: &str,
    what: &str,
    known: impl Fn(&str) -> bool,
) -> Result<String, String> {
    if value == "random" || known(value) {
        Ok(value.to_string())
    } else {
        Err(format!(
            "unknown {what} {value:?}; expected a built-in name or \"random\""
        ))
    }
}

/// Accepts `R,G,B`.
fn parse_colour(value: &str) -> Result<Rgb, String> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(format!(
            "expects three comma-separated values like 32,96,255, got {value:?}"
        ));
    }
    let channel = |text: &&str| {
        text.parse::<u8>()
            .map_err(|_| format!("channel {text:?} is not 0-255"))
    };
    Ok(Rgb::new(
        channel(&parts[0])?,
        channel(&parts[1])?,
        channel(&parts[2])?,
    ))
}

/// Accepts 1 to 240. 0 would silently become 1 fps; an absurd value makes the
/// period vanish and turns the loop into an unthrottled write storm. The board
/// measured ~69 fps, so 240 leaves headroom.
fn parse_fps(value: &str) -> Result<u32, String> {
    let fps: u32 = value
        .parse()
        .map_err(|_| format!("expects a whole number, got {value:?}"))?;
    if !(1..=240).contains(&fps) {
        return Err(format!("expects 1 to 240, got {fps}"));
    }
    Ok(fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, clap::Error> {
        parse_args(std::iter::once("dygmarave").chain(list.iter().copied()))
    }

    #[test]
    fn defaults_measure_without_specifying_anything() {
        let parsed = args(&[]).unwrap();
        // These are the clap attribute defaults. Asserting each field is the guard
        // that the parser and the documented defaults stay in step; a duplicate
        // `Default` impl would restate them and could drift.
        assert_eq!(parsed.port, None);
        assert_eq!(parsed.colour, Rgb::new(32, 96, 255));
        assert_eq!(parsed.hold_ms, 3_000);
        assert_eq!(parsed.frames, 120);
        assert!(!parsed.probe_audio);
        assert!(!parsed.probe_columns);
        assert!(!parsed.probe_rows);
        assert!(!parsed.probe_gradient);
        assert!(!parsed.probe_thumbs);
        assert!(!parsed.probe_spectrum);
        assert!(!parsed.live);
        assert_eq!(parsed.layout, None);
        assert_eq!(parsed.palette, None);
        assert_eq!(parsed.motion, None);
        assert_eq!(parsed.seed, None);
        assert!(!parsed.quiet);
        assert_eq!(parsed.fps, None);
        assert_eq!(parsed.seconds, 5);
        assert_eq!(parsed.source, None);
    }

    #[test]
    fn parses_every_flag() {
        let parsed = args(&[
            "--port",
            "/dev/ttyACM9",
            "--colour",
            "1,2,3",
            "--hold-ms",
            "500",
            "--frames",
            "60",
        ])
        .unwrap();
        assert_eq!(parsed.port.as_deref(), Some("/dev/ttyACM9"));
        assert_eq!(parsed.colour, Rgb::new(1, 2, 3));
        assert_eq!(parsed.hold_ms, 500);
        assert_eq!(parsed.frames, 60);
    }

    #[test]
    fn parses_the_live_flags() {
        let parsed = args(&["--live", "--fps", "60"]).unwrap();
        assert!(parsed.live);
        assert_eq!(parsed.fps, Some(60));
        assert_eq!(
            args(&[]).unwrap().fps,
            None,
            "None means use the config file's target"
        );
    }

    #[test]
    fn parses_the_look_axis_flags_seed_and_quiet() {
        let parsed = args(&[
            "--layout",
            "rings",
            "--palette",
            "ice",
            "--motion",
            "sweep",
            "--seed",
            "42",
            "--quiet",
        ])
        .unwrap();
        assert_eq!(parsed.layout.as_deref(), Some("rings"));
        assert_eq!(parsed.palette.as_deref(), Some("ice"));
        assert_eq!(parsed.motion.as_deref(), Some("sweep"));
        assert_eq!(parsed.seed, Some(42));
        assert!(parsed.quiet);

        for flag in ["--layout", "--palette", "--motion"] {
            let parsed = args(&[flag, "random"]).unwrap();
            match flag {
                "--layout" => assert_eq!(parsed.layout.as_deref(), Some("random")),
                "--palette" => assert_eq!(parsed.palette.as_deref(), Some("random")),
                _ => assert_eq!(parsed.motion.as_deref(), Some("random")),
            }
        }
    }

    #[test]
    fn invalid_look_axis_names_exit_two() {
        for (flag, value) in [
            ("--layout", "wobble"),
            ("--palette", "rust"),
            ("--motion", "shimmer"),
            ("--layout", "none"),
            ("--palette", "random-please"),
        ] {
            let error = args(&[flag, value]).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{flag} {value} must be a usage error");
            assert!(error.to_string().contains("unknown"));
        }
    }

    #[test]
    fn a_non_numeric_seed_is_rejected() {
        assert!(args(&["--seed", "later"]).is_err());
    }

    #[test]
    fn rejects_an_fps_outside_the_supported_range() {
        assert!(args(&["--fps", "0"])
            .unwrap_err()
            .to_string()
            .contains("1 to 240"));
        assert!(args(&["--fps", "241"])
            .unwrap_err()
            .to_string()
            .contains("1 to 240"));
        assert!(args(&["--fps", "240"]).is_ok());
    }

    #[test]
    fn rejects_unknown_flags() {
        let error = args(&["--wobble"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        assert_eq!(error.exit_code(), 2, "usage errors exit 2");
    }

    #[test]
    fn rejects_a_colour_without_three_channels() {
        assert!(args(&["--colour", "1,2"])
            .unwrap_err()
            .to_string()
            .contains("three"));
    }

    #[test]
    fn rejects_a_channel_above_byte_range() {
        assert!(args(&["--colour", "0,0,256"])
            .unwrap_err()
            .to_string()
            .contains("0-255"));
    }

    #[test]
    fn rejects_a_flag_missing_its_value() {
        assert!(args(&["--port"]).unwrap_err().to_string().contains("value"));
    }

    #[test]
    fn parses_the_audio_probe_flags() {
        let parsed = args(&["--probe-audio", "--seconds", "2", "--source", "pipewire:42"]).unwrap();
        assert!(parsed.probe_audio);
        assert_eq!(parsed.seconds, 2);
        assert_eq!(parsed.source.as_deref(), Some("pipewire:42"));
    }

    #[test]
    fn the_pattern_flags_are_independent_of_the_audio_probes() {
        let parsed = args(&["--probe-columns"]).unwrap();
        assert!(parsed.probe_columns);
        assert!(!parsed.probe_audio);
        assert!(args(&["--columns"]).unwrap().probe_columns);
        assert!(args(&["--probe-rows"]).unwrap().probe_rows);
        assert!(args(&["--rows"]).unwrap().probe_rows);
        assert!(args(&["--probe-gradient"]).unwrap().probe_gradient);
        assert!(args(&["--gradient"]).unwrap().probe_gradient);
        assert!(args(&["--probe-thumbs"]).unwrap().probe_thumbs);
        assert!(args(&["--thumbs"]).unwrap().probe_thumbs);
        assert!(args(&["--thumb-pattern"]).unwrap().probe_thumbs);
    }

    #[test]
    fn the_spectrum_probe_implies_the_audio_probe() {
        let parsed = args(&["--probe-spectrum"]).unwrap();
        assert!(parsed.probe_audio, "spectrum needs audio capture");
        assert!(parsed.probe_spectrum);
    }

    #[test]
    fn audio_probe_defaults_to_five_seconds_of_choosing_for_itself() {
        let parsed = args(&["--probe-audio"]).unwrap();
        assert_eq!(parsed.seconds, 5);
        assert_eq!(parsed.source, None);
    }

    #[test]
    fn rejects_a_non_numeric_duration() {
        assert!(args(&["--seconds", "soon"])
            .unwrap_err()
            .to_string()
            .contains("seconds"));
    }

    #[test]
    fn the_live_and_probe_flags_conflict() {
        // The hand-rolled parser spelled this out in a string whose wrap broke:
        // 21 literal spaces printed mid-sentence. The conflict is now declarative.
        for conflicting in ["--probe-audio", "--probe-spectrum"] {
            let error = args(&["--live", conflicting]).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "--live and {conflicting} must conflict"
            );
            assert_eq!(error.exit_code(), 2, "usage errors exit 2");
        }
    }

    #[test]
    fn help_calls_the_probe_flags_static_hardware_probes() {
        let help = args(&["--help"]).unwrap_err().to_string();
        assert!(help.contains("static hardware probe"));
        assert!(help.contains("--probe-columns"));
        assert!(help.contains("--probe-thumbs"));
        assert!(help.contains("--seed"));
        assert!(help.contains("--quiet"));
    }

    #[test]
    fn help_and_version_exit_through_clap() {
        for flag in ["--help", "-h", "--version"] {
            let error = args(&[flag]).unwrap_err();
            assert!(
                matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ),
                "{flag} must be handled by clap"
            );
            assert_eq!(error.exit_code(), 0, "{flag} must exit successfully");
        }
    }
}
