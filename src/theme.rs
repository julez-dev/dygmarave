//! Colour values and the `led.theme` wire format.
//!
//! The Focus protocol is plain-text and space-separated: a full frame is
//! `led.theme R G B R G B …`, one triplet per LED index, and the device's reply
//! to a bare `led.theme` is the same shape of triplet list. Both directions are
//! handled here so nothing else has to know the format.

use std::fmt;

/// An 8-bit RGB colour, matching the device's per-LED precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Blends toward `other` by `t`, rounding each channel to the nearest byte.
    ///
    /// `t` is clamped, so out-of-range callers get an endpoint rather than a
    /// wrapped colour. This is the same blend and rounding the colour ramp uses,
    /// and the crossfade in ticket 03 will use it too.
    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
        )
    }
}

impl Rgb {
    /// How many distinct colours a colour list contains.
    pub fn distinct(colors: &[Rgb]) -> usize {
        let mut unique = colors.to_vec();
        unique.sort_by_key(|color| (color.r, color.g, color.b));
        unique.dedup();
        unique.len()
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.r, self.g, self.b)
    }
}

/// Why a device reply could not be read as a theme.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    /// The reply contained no colour values at all.
    #[error("the device returned no colour values")]
    Empty,
    /// The value count is not a whole number of RGB triplets.
    #[error("the device returned {values} values, which is not a whole number of RGB triplets")]
    Ragged { values: usize },
    /// A value was not a byte-sized integer.
    #[error("the device returned a value that is not a byte: {token:?}")]
    BadValue { token: String },
}

/// Parses a `led.theme` reply into one colour per LED index.
///
/// The verifier's lone `.` terminator is tolerated here rather than assumed to
/// have been stripped, so a framing change upstream cannot silently corrupt the
/// colour list.
pub fn parse_theme_reply(reply: &str) -> Result<Vec<Rgb>, ThemeError> {
    let values = reply
        .split_whitespace()
        .filter(|token| *token != ".")
        .map(|token| {
            token.parse::<u8>().map_err(|_| ThemeError::BadValue {
                token: token.to_string(),
            })
        })
        .collect::<Result<Vec<u8>, ThemeError>>()?;

    if values.is_empty() {
        return Err(ThemeError::Empty);
    }
    if values.len() % 3 != 0 {
        return Err(ThemeError::Ragged {
            values: values.len(),
        });
    }

    Ok(values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|rgb| Rgb::new(rgb[0], rgb[1], rgb[2]))
        .collect())
}

/// Encodes a colour list as the body of a `led.theme` command, without a
/// trailing newline (the link adds framing).
pub fn encode_theme(colors: &[Rgb]) -> String {
    let mut out = String::with_capacity(10 + colors.len() * 12);
    out.push_str("led.theme");
    for color in colors {
        out.push(' ');
        out.push_str(&color.to_string());
    }
    out
}

/// A full frame filled with one colour, sized to the board's LED count.
pub fn uniform(count: usize, color: Rgb) -> Vec<Rgb> {
    vec![color; count]
}

/// One encoded `led.theme` command, ready to write.
///
/// ADR-0001 forbids anything but `led.theme` in the frame loop, and warns that
/// the restriction is easy to regress because a smaller EEPROM-backed command
/// looks like an optimisation. A frame therefore cannot be built out of an
/// arbitrary string: the only way to obtain one is from colours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    body: String,
    leds: usize,
}

impl Frame {
    pub fn from_colors(colors: &[Rgb]) -> Self {
        Self {
            body: encode_theme(colors),
            leds: colors.len(),
        }
    }

    pub fn uniform(count: usize, color: Rgb) -> Self {
        Self::from_colors(&uniform(count, color))
    }

    /// The command body, without the framing newline.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// How many LEDs this frame addresses.
    pub fn leds(&self) -> usize {
        self.leds
    }

    /// Bytes on the wire, including the newline the link adds.
    pub fn byte_len(&self) -> usize {
        self.body.len() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_colour_per_triplet() {
        let colors = parse_theme_reply("1 2 3 4 5 6").unwrap();
        assert_eq!(colors, vec![Rgb::new(1, 2, 3), Rgb::new(4, 5, 6)]);
    }

    #[test]
    fn tolerates_terminator_and_whitespace() {
        let colors = parse_theme_reply("  1 2 3\r\n.\r\n").unwrap();
        assert_eq!(colors, vec![Rgb::new(1, 2, 3)]);
    }

    #[test]
    fn rejects_ragged_value_count() {
        assert_eq!(
            parse_theme_reply("1 2 3 4").unwrap_err(),
            ThemeError::Ragged { values: 4 }
        );
    }

    #[test]
    fn rejects_empty_reply() {
        assert_eq!(
            parse_theme_reply("  . \r\n").unwrap_err(),
            ThemeError::Empty
        );
    }

    #[test]
    fn rejects_out_of_range_value() {
        assert_eq!(
            parse_theme_reply("1 2 300").unwrap_err(),
            ThemeError::BadValue {
                token: "300".to_string()
            }
        );
    }

    #[test]
    fn encodes_a_frame_the_device_can_parse() {
        assert_eq!(
            encode_theme(&[Rgb::new(1, 2, 3), Rgb::new(255, 0, 10)]),
            "led.theme 1 2 3 255 0 10"
        );
    }

    #[test]
    fn a_frame_counts_its_newline_as_wire_bytes() {
        let frame = Frame::uniform(2, Rgb::new(1, 2, 3));
        assert_eq!(frame.body(), "led.theme 1 2 3 1 2 3");
        assert_eq!(frame.byte_len(), frame.body().len() + 1);
    }

    #[test]
    fn lerp_blends_and_rounds_each_channel() {
        let black = Rgb::new(0, 0, 0);
        let red = Rgb::new(255, 0, 0);
        let white = Rgb::new(255, 255, 255);
        assert_eq!(black.lerp(red, 0.0), black);
        assert_eq!(black.lerp(red, 1.0), red);
        assert_eq!(red.lerp(white, 0.5), Rgb::new(255, 128, 128));
        assert_eq!(black.lerp(red, -1.0), black, "t is clamped");
        assert_eq!(black.lerp(red, 2.0), red, "t is clamped");
    }

    #[test]
    fn distinct_counts_unique_colours() {
        let colors = vec![Rgb::new(1, 1, 1), Rgb::new(1, 1, 1), Rgb::new(2, 2, 2)];
        assert_eq!(Rgb::distinct(&colors), 2);
    }

    #[test]
    fn encoding_round_trips_through_parsing() {
        let frame = uniform(132, Rgb::new(32, 96, 255));
        let encoded = encode_theme(&frame);
        let body = encoded.strip_prefix("led.theme ").unwrap();
        assert_eq!(parse_theme_reply(body).unwrap(), frame);
    }
}
