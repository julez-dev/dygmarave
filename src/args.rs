//! Command-line arguments.
//!
//! Deliberately hand-rolled: ticket 01 needs four flags, and the config module
//! (ticket 09) will own the real surface.

use crate::theme::Rgb;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// Explicit port, skipping discovery.
    pub port: Option<String>,
    /// Colour to paint and measure with.
    pub colour: Rgb,
    /// How long to hold the colour before checking it survived.
    pub hold_ms: u64,
    /// How many frames to write when measuring throughput.
    pub frames: u32,
    pub help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            port: None,
            colour: Rgb::new(32, 96, 255),
            hold_ms: 3_000,
            frames: 120,
            help: false,
        }
    }
}

pub const USAGE: &str = "\
dygmarave — prove the LED path and measure the frame budget

Usage: dygmarave [OPTIONS]

Options:
  --port <PATH>       serial port to use, skipping USB discovery
  --colour <R,G,B>    colour to paint and measure with (default 32,96,255)
  --hold-ms <MS>      hold the colour this long before checking it (default 3000)
  --frames <N>        frames to write when measuring (default 120)
  -h, --help          show this help

The board is found by USB identity, not by a fixed port name. The LED count is
read from the board itself. The baseline theme and LED mode are restored on exit
and when Ctrl-C is pressed once; pressing Ctrl-C a second time aborts without
restoring.";

pub fn parse<I>(args: I) -> Result<Args, String>
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = Args::default();
    let mut args = args.into_iter();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "-h" | "--help" => parsed.help = true,
            "--port" => parsed.port = Some(next_value(&mut args, &flag)?),
            "--hold-ms" => {
                let value = next_value(&mut args, &flag)?;
                parsed.hold_ms = value.parse().map_err(|_| {
                    format!("--hold-ms expects a whole number of milliseconds, got {value:?}")
                })?;
            }
            "--frames" => {
                let value = next_value(&mut args, &flag)?;
                parsed.frames = value
                    .parse()
                    .map_err(|_| format!("--frames expects a whole number, got {value:?}"))?;
            }
            "--colour" | "--color" => {
                let value = next_value(&mut args, &flag)?;
                parsed.colour = parse_colour(&value)?;
            }
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }

    Ok(parsed)
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} expects a value"))
}

/// Accepts `R,G,B`.
fn parse_colour(value: &str) -> Result<Rgb, String> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(format!(
            "--colour expects three comma-separated values like 32,96,255, got {value:?}"
        ));
    }
    let channel = |text: &&str| {
        text.parse::<u8>()
            .map_err(|_| format!("--colour channel {text:?} is not 0-255"))
    };
    Ok(Rgb::new(
        channel(&parts[0])?,
        channel(&parts[1])?,
        channel(&parts[2])?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse(list.iter().map(|value| value.to_string()))
    }

    #[test]
    fn defaults_measure_without_specifying_anything() {
        let parsed = args(&[]).unwrap();
        assert_eq!(parsed, Args::default());
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
    fn rejects_unknown_flags() {
        assert!(args(&["--wobble"])
            .unwrap_err()
            .contains("unknown argument"));
    }

    #[test]
    fn rejects_a_colour_without_three_channels() {
        assert!(args(&["--colour", "1,2"]).unwrap_err().contains("three"));
    }

    #[test]
    fn rejects_a_channel_above_byte_range() {
        assert!(args(&["--colour", "0,0,256"])
            .unwrap_err()
            .contains("0-255"));
    }

    #[test]
    fn rejects_a_flag_missing_its_value() {
        assert!(args(&["--port"]).unwrap_err().contains("expects a value"));
    }

    #[test]
    fn help_short_circuits() {
        assert!(args(&["--help"]).unwrap().help);
        assert!(args(&["-h"]).unwrap().help);
    }
}
