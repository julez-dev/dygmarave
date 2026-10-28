//! Named colour palettes: energy levels mapped to colour ramps.
//!
//! A palette is a sorted list of `(position, colour)` stops, interpolated
//! piecewise. The v0.1 renderer had one hard-coded ramp; `ember` is that ramp,
//! preserved byte-for-byte so an empty config still produces the same frames.
//!
//! The black floor is applied after sampling: a level below the floor is black
//! regardless of the palette. That is what lets a decaying band go dark instead
//! of glowing faintly forever, and it applies to every palette equally.

use std::fmt;
use std::str::FromStr;

use crate::theme::Rgb;

/// One palette stop: a position in `0.0..=1.0` and the colour at that position.
pub type Stop = (f32, Rgb);

/// The eight built-in palettes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Palette {
    #[default]
    Ember,
    Ice,
    Magma,
    Neon,
    Mono,
    Spectrum,
    Solar,
    Forest,
}

/// The v0.1 ramp: black through red and orange to white.
///
/// The positions are the ones ADR-0003 and the spec record. They also reproduce
/// the old renderer's segment boundaries exactly; see [`sample`].
const EMBER_STOPS: [Stop; 4] = [
    (0.00, Rgb::BLACK),
    (0.35, Rgb::new(255, 0, 0)),
    (0.65, Rgb::new(255, 128, 0)),
    (1.00, Rgb::new(255, 255, 255)),
];

const ICE_STOPS: [Stop; 4] = [
    (0.00, Rgb::BLACK),
    (0.35, Rgb::new(0, 32, 120)),
    (0.65, Rgb::new(0, 180, 255)),
    (1.00, Rgb::new(255, 255, 255)),
];

const MAGMA_STOPS: [Stop; 5] = [
    (0.00, Rgb::BLACK),
    (0.30, Rgb::new(64, 0, 96)),
    (0.60, Rgb::new(200, 0, 64)),
    (0.80, Rgb::new(255, 160, 0)),
    (1.00, Rgb::new(255, 255, 220)),
];

const NEON_STOPS: [Stop; 4] = [
    (0.00, Rgb::BLACK),
    (0.40, Rgb::new(120, 0, 200)),
    (0.70, Rgb::new(255, 0, 128)),
    (1.00, Rgb::new(0, 255, 255)),
];

const MONO_STOPS: [Stop; 4] = [
    (0.00, Rgb::BLACK),
    (0.35, Rgb::new(64, 64, 64)),
    (0.65, Rgb::new(160, 160, 160)),
    (1.00, Rgb::new(255, 255, 255)),
];

/// Deliberately non-monotonic: it is a spectrum, not a brightness ramp.
const SPECTRUM_STOPS: [Stop; 8] = [
    (0.00, Rgb::BLACK),
    (0.15, Rgb::new(148, 0, 255)),
    (0.30, Rgb::new(0, 64, 255)),
    (0.45, Rgb::new(0, 220, 255)),
    (0.60, Rgb::new(0, 255, 96)),
    (0.75, Rgb::new(255, 255, 0)),
    (0.90, Rgb::new(255, 64, 0)),
    (1.00, Rgb::new(255, 255, 255)),
];

const SOLAR_STOPS: [Stop; 5] = [
    (0.00, Rgb::BLACK),
    (0.35, Rgb::new(80, 40, 0)),
    (0.65, Rgb::new(220, 150, 0)),
    (0.85, Rgb::new(255, 230, 120)),
    (1.00, Rgb::new(255, 255, 255)),
];

const FOREST_STOPS: [Stop; 5] = [
    (0.00, Rgb::BLACK),
    (0.35, Rgb::new(0, 64, 24)),
    (0.65, Rgb::new(32, 200, 48)),
    (0.85, Rgb::new(160, 255, 96)),
    (1.00, Rgb::new(255, 255, 255)),
];

impl Palette {
    /// Every palette, in the order the spec lists them.
    pub const ALL: [Palette; 8] = [
        Palette::Ember,
        Palette::Ice,
        Palette::Magma,
        Palette::Neon,
        Palette::Mono,
        Palette::Spectrum,
        Palette::Solar,
        Palette::Forest,
    ];

    /// The config spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Palette::Ember => "ember",
            Palette::Ice => "ice",
            Palette::Magma => "magma",
            Palette::Neon => "neon",
            Palette::Mono => "mono",
            Palette::Spectrum => "spectrum",
            Palette::Solar => "solar",
            Palette::Forest => "forest",
        }
    }

    /// Parses a config spelling, case-sensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|palette| palette.name() == name)
    }

    /// The palette's stops, sorted by ascending position.
    pub fn stops(self) -> &'static [Stop] {
        match self {
            Palette::Ember => &EMBER_STOPS,
            Palette::Ice => &ICE_STOPS,
            Palette::Magma => &MAGMA_STOPS,
            Palette::Neon => &NEON_STOPS,
            Palette::Mono => &MONO_STOPS,
            Palette::Spectrum => &SPECTRUM_STOPS,
            Palette::Solar => &SOLAR_STOPS,
            Palette::Forest => &FOREST_STOPS,
        }
    }
}

impl fmt::Display for Palette {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a palette name was not recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPalette {
    pub name: String,
}

impl fmt::Display for UnknownPalette {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let expected = Palette::ALL.map(|palette| palette.name()).join(", ");
        write!(
            f,
            "unknown palette {:?}; expected one of {expected}",
            self.name
        )
    }
}

impl std::error::Error for UnknownPalette {}

impl FromStr for Palette {
    type Err = UnknownPalette;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_name(name).ok_or_else(|| UnknownPalette {
            name: name.to_string(),
        })
    }
}

/// Samples a palette at `level`, with `floor` applied after the level is clamped.
///
/// A level below `floor` is black. Otherwise the two neighbouring stops are
/// interpolated and rounded to bytes, the same rounding the v0.1 renderer used.
///
/// Stop positions are decimal fractions, so their span is measured in
/// thousandths. Doing it that way keeps the legacy ember spans (`0.35`, `0.30`,
/// `0.35`) exact: subtracting the `f32` positions directly would give
/// `0.649999976 - 0.349999994`, one bit off from the old literal `0.30`, and
/// change the final colour at a few levels. Ember must stay byte-identical.
pub fn sample(palette: Palette, level: f32, floor: f32) -> Rgb {
    let level = level.clamp(0.0, 1.0);
    if level < floor {
        return Rgb::BLACK;
    }

    let stops = palette.stops();
    debug_assert!(stops.len() >= 2, "a palette needs at least two stops");

    // Pick the segment whose upper stop is strictly above the level. At a stop
    // boundary this selects the segment starting there, so the sampled colour is
    // exactly that stop. At 1.0 it selects the last segment.
    let mut segment = stops.len() - 2;
    for index in 0..stops.len() - 1 {
        if level < stops[index + 1].0 {
            segment = index;
            break;
        }
    }

    let (start, from) = stops[segment];
    let (end, to) = stops[segment + 1];
    let span = (whole_thousandths(end) - whole_thousandths(start)) as f32 / 1000.0;
    let t = if span > 0.0 {
        ((level - start) / span).clamp(0.0, 1.0)
    } else {
        1.0
    };

    from.lerp(to, t)
}

/// A stop position as an integer number of thousandths.
fn whole_thousandths(position: f32) -> i32 {
    (position * 1000.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The v0.1 ramp, copied verbatim as the oracle ember must match.
    fn v0_1_ramp(level: f32, floor: f32) -> Rgb {
        const BLACK: Rgb = Rgb::BLACK;
        const RED: Rgb = Rgb::new(255, 0, 0);
        const ORANGE: Rgb = Rgb::new(255, 128, 0);
        const WHITE: Rgb = Rgb::new(255, 255, 255);

        let level = level.clamp(0.0, 1.0);
        if level < floor {
            return BLACK;
        }

        let (from, to, start, span) = if level < 0.35 {
            (BLACK, RED, 0.0, 0.35)
        } else if level < 0.65 {
            (RED, ORANGE, 0.35, 0.30)
        } else {
            (ORANGE, WHITE, 0.65, 0.35)
        };
        let t = ((level - start) / span).clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb::new(mix(from.r, to.r), mix(from.g, to.g), mix(from.b, to.b))
    }

    #[test]
    fn every_palette_has_a_config_name_that_round_trips() {
        assert_eq!(Palette::ALL.len(), 8);
        for palette in Palette::ALL {
            let name = palette.name();
            assert_eq!(Palette::from_name(name), Some(palette));
            assert_eq!(name.parse::<Palette>(), Ok(palette));
            assert_eq!(palette.to_string(), name);
        }
        assert_eq!(Palette::from_name("nope"), None);
        assert_eq!(
            "nope".parse::<Palette>(),
            Err(UnknownPalette {
                name: "nope".to_string()
            })
        );
    }

    #[test]
    fn the_default_palette_is_ember() {
        assert_eq!(Palette::default(), Palette::Ember);
    }

    #[test]
    fn stops_are_sorted_from_zero_to_one() {
        for palette in Palette::ALL {
            let stops = palette.stops();
            assert!(stops.len() >= 2, "{palette} needs at least two stops");
            assert_eq!(stops[0].0, 0.0, "{palette} must start at 0.0");
            assert_eq!(stops[stops.len() - 1].0, 1.0, "{palette} must end at 1.0");
            for pair in stops.windows(2) {
                assert!(
                    pair[0].0 < pair[1].0,
                    "{palette}: positions {} and {} are not ascending",
                    pair[0].0,
                    pair[1].0
                );
            }
            for (position, _) in stops {
                let snapped = whole_thousandths(*position) as f32 / 1000.0;
                assert_eq!(
                    snapped, *position,
                    "{palette} stop {position} is not a whole thousandth; interpolation spans assume they are"
                );
            }
        }
    }

    #[test]
    fn sampling_the_endpoints_returns_black_and_the_last_stop() {
        for palette in Palette::ALL {
            let stops = palette.stops();
            assert_eq!(sample(palette, 0.0, 0.0), Rgb::BLACK);
            assert_eq!(sample(palette, 1.0, 0.0), stops[stops.len() - 1].1);
        }
    }

    #[test]
    fn the_floor_blacks_levels_below_it_and_not_at_it() {
        for palette in Palette::ALL {
            assert_eq!(
                sample(palette, 0.019, 0.02),
                Rgb::BLACK,
                "{palette} must be black below the floor"
            );
            assert_ne!(
                sample(palette, 0.02, 0.02),
                Rgb::BLACK,
                "{palette} must light at the floor boundary"
            );
        }
    }

    #[test]
    fn ember_reproduces_the_v0_1_ramp_at_every_sampled_level() {
        // Sweep the levels the AGC can produce, plus the floor boundary and the
        // out-of-range clamping the old ramp handled.
        for step in 0..=1000 {
            let level = step as f32 / 1000.0;
            assert_eq!(
                sample(Palette::Ember, level, 0.02),
                v0_1_ramp(level, 0.02),
                "ember differs from the v0.1 ramp at level {level}"
            );
        }
        for level in [-1.0, 0.0, 0.019, 0.02, 0.35, 0.65, 0.999, 1.0, 2.0] {
            assert_eq!(
                sample(Palette::Ember, level, 0.02),
                v0_1_ramp(level, 0.02),
                "ember differs from the v0.1 ramp at level {level}"
            );
        }
        for floor in [0.0, 0.02, 0.1, 0.5] {
            for step in 0..=200 {
                let level = step as f32 / 200.0;
                assert_eq!(
                    sample(Palette::Ember, level, floor),
                    v0_1_ramp(level, floor),
                    "ember differs from the v0.1 ramp at level {level} with floor {floor}"
                );
            }
        }
    }

    #[test]
    fn the_ember_ramp_runs_from_black_through_warm_to_white() {
        assert_eq!(sample(Palette::Ember, 0.0, 0.02), Rgb::BLACK);
        assert_eq!(sample(Palette::Ember, 1.0, 0.02), Rgb::new(255, 255, 255));
        assert_eq!(sample(Palette::Ember, 0.4, 0.02), Rgb::new(255, 21, 0));
        assert_eq!(sample(Palette::Ember, 0.7, 0.02), Rgb::new(255, 146, 36));
    }

    #[test]
    fn the_ramp_floors_a_decaying_level_to_black() {
        assert_eq!(sample(Palette::Ember, 0.0, 0.02), Rgb::BLACK);
        assert_eq!(sample(Palette::Ember, 0.019, 0.02), Rgb::BLACK);
        assert_ne!(sample(Palette::Ember, 0.02, 0.02), Rgb::BLACK);
    }

    #[test]
    fn the_ramp_clamps_out_of_range_input() {
        assert_eq!(sample(Palette::Ember, -1.0, 0.02), Rgb::BLACK);
        assert_eq!(sample(Palette::Ember, 2.0, 0.02), Rgb::new(255, 255, 255));
    }

    #[test]
    fn every_palette_except_spectrum_brightens_monotonically() {
        // Spectrum is deliberately non-monotonic. Every other ramp is designed to
        // read as a single brightening arc, so a reversal would be a defect.
        for palette in Palette::ALL {
            if palette == Palette::Spectrum {
                continue;
            }
            let mut previous = 0u32;
            for step in 0..=100 {
                let colour = sample(palette, step as f32 / 100.0, 0.0);
                let brightness = colour.r as u32 + colour.g as u32 + colour.b as u32;
                assert!(
                    brightness >= previous,
                    "{palette} reversed at {step}%: {brightness} < {previous}"
                );
                previous = brightness;
            }
        }
    }

    #[test]
    fn spectrum_is_allowed_to_dip_in_brightness() {
        let brightness = |level: f32| {
            let colour = sample(Palette::Spectrum, level, 0.0);
            colour.r as u32 + colour.g as u32 + colour.b as u32
        };
        assert!(
            brightness(0.15) > brightness(0.30),
            "spectrum should step down from violet to blue, or this test is pointless"
        );
    }
}
