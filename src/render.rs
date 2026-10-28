//! Composition: place bands, colour them, modulate them, paint the thumbs.
//!
//! [`crate::dynamics::Dynamics`] supplies per-band levels, thumb levels and beat
//! signals. This module only turns those signals into a colour buffer: the layout
//! chooses which key carries each band, the palette chooses the colour, motion
//! modulates the matrix, and the thumb clusters keep their level glow and white
//! beat flash.

use std::time::{Duration, Instant};

use crate::dynamics::Dynamics;
#[cfg(test)]
use crate::layout::Layout;
use crate::layout::{self, Cell, Half};
use crate::motion::MotionState;
use crate::palette::{self, Palette};
use crate::theme::Rgb;

/// A fixed look: which layout places the bands and which palette colours them.
///
/// The default is the v0.1 look, `snake` + `ember`. Motion is deliberately not
/// stored here: it carries a clock and event state, so `compose` takes an
/// explicit `&MotionState` alongside the look. That is what lets a crossfade
/// render two selections in one frame without advancing events twice.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub layout: Layout,
    pub palette: Palette,
}

#[cfg(test)]
impl Default for Look {
    fn default() -> Self {
        Self {
            layout: Layout::Snake,
            palette: Palette::Ember,
        }
    }
}

/// Paints the dynamics onto the grid: band *n* of a half lights that half's
/// *n*-th LED in the selected layout, coloured by the selected palette and then
/// modulated by `motion`. The thumb clusters carry their channel's level in the
/// same palette and flash white on a beat; motion never touches them. LEDs
/// without a band — underglow and the top bands the half has no keys for — stay
/// black.
#[cfg(test)]
pub fn compose(dynamics: &Dynamics, count: usize, look: &Look, motion: &MotionState) -> Vec<Rgb> {
    let left = layout::band_cells_for(&look.layout, Half::Left);
    let right = layout::band_cells_for(&look.layout, Half::Right);
    compose_bands(dynamics, count, look.palette, &left, &right, motion)
}

/// The look engine's render primitive: paints using already-resolved band tables.
///
/// `LookState` passes its cached tables so a fade frame does not recompute a
/// layout permutation.
pub(crate) fn compose_bands(
    dynamics: &Dynamics,
    count: usize,
    palette: Palette,
    left_bands: &[(Cell, u8)],
    right_bands: &[(Cell, u8)],
    motion: &MotionState,
) -> Vec<Rgb> {
    let mut colours = vec![Rgb::BLACK; count];
    let config = dynamics.config();
    for (half, preferred, bands) in [(Half::Left, 0, left_bands), (Half::Right, 1, right_bands)] {
        // A mono capture feeds both halves; a stereo one feeds its own side.
        let channel = if dynamics.channels() == 1 {
            0
        } else {
            preferred
        };
        for (band, (cell, led)) in bands.iter().enumerate() {
            if band >= dynamics.bands() {
                break;
            }
            if let Some(colour) = colours.get_mut(*led as usize) {
                let sampled = palette::sample(palette, dynamics.level(channel, band), config.floor);
                *colour = motion.modulate(sampled, *cell, half, dynamics);
            }
        }

        // The thumb cluster carries the channel's level, and flashes white on a
        // beat before settling back. It is deliberately separate from the matrix
        // bands so the spectrum stays readable while the thumbs carry rhythm.
        let thumb_colour = if config.beat_flash && dynamics.is_flashing(channel) {
            Rgb::new(255, 255, 255)
        } else if config.thumb_glow {
            palette::sample(palette, dynamics.thumb_level(channel), config.floor)
        } else {
            Rgb::BLACK
        };
        for led in layout::thumb_leds(half) {
            if let Some(colour) = colours.get_mut(*led as usize) {
                *colour = thumb_colour;
            }
        }
    }
    colours
}

/// Keeps frame pacing steady without drift.
///
/// Sleeping for a fixed period every frame accumulates the duration of the work
/// itself, so the visualizer slowly falls behind the music. This advances a
/// deadline by exactly one period instead, and resynchronises after a stall
/// rather than trying to catch up by writing a burst of frames.
#[derive(Debug, Clone)]
pub struct Pacer {
    period: Duration,
    next: Instant,
    dropped: u64,
}

impl Pacer {
    pub fn new(period: Duration, start: Instant) -> Self {
        Self {
            period,
            next: start + period,
            dropped: 0,
        }
    }

    /// How long to wait before the next frame, and whether this call gave up on
    /// a deadline that was already unreachable.
    pub fn wait(&mut self, now: Instant) -> (Duration, bool) {
        if now > self.next + self.period {
            // More than a whole frame late: catching up would mean writing a
            // burst, so give up the missed deadline and resynchronise.
            self.next = now + self.period;
            self.dropped += 1;
            return (self.period, true);
        }
        let wait = self.next.saturating_duration_since(now);
        self.next += self.period;
        (wait, false)
    }

    /// Changes the period, realigning the next deadline to one new period from
    /// `now`.
    ///
    /// A deliberate rate change is not a missed deadline, so it must not count as
    /// a dropped frame; the schedule simply continues on the new grid.
    pub fn set_period(&mut self, period: Duration, now: Instant) {
        self.period = period;
        self.next = now + period;
    }

    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamics::{DynamicsConfig, BASS_BANDS, FLOOR};
    use crate::spectrum::{Analyzer, BandPlan, Spectrum, BAND_COUNT, FFT_SIZE};

    fn spectrum_with(values: &[f32]) -> Spectrum {
        let mut spectrum = Spectrum::new(1, values.len());
        spectrum.set_bands(0, values);
        spectrum
    }

    fn spectral_flat(bands: usize, energy: f32) -> Spectrum {
        spectrum_with(&vec![energy; bands])
    }

    #[test]
    fn the_default_palette_runs_from_black_through_warm_to_white() {
        assert_eq!(palette::sample(Palette::Ember, 0.0, FLOOR), Rgb::BLACK);
        assert_eq!(
            palette::sample(Palette::Ember, 1.0, FLOOR),
            Rgb::new(255, 255, 255)
        );
        assert_eq!(
            palette::sample(Palette::Ember, 0.4, FLOOR),
            Rgb::new(255, 21, 0)
        );
        assert_eq!(
            palette::sample(Palette::Ember, 0.7, FLOOR),
            Rgb::new(255, 146, 36)
        );

        // Brightness never falls as the level rises, so the picture reads as a
        // single ramp rather than reversing part way up.
        let mut previous = 0u32;
        for step in 0..=100 {
            let colour = palette::sample(Palette::Ember, step as f32 / 100.0, FLOOR);
            let brightness = colour.r as u32 + colour.g as u32 + colour.b as u32;
            assert!(brightness >= previous, "ramp reversed at {step}%");
            previous = brightness;
        }
    }

    #[test]
    fn the_ramp_floors_a_decaying_level_to_black() {
        // A fade approaches zero without arriving; without the floor the board
        // would glow faintly red forever after the music stops.
        assert_eq!(palette::sample(Palette::Ember, 0.0, FLOOR), Rgb::BLACK);
        assert_eq!(palette::sample(Palette::Ember, 0.019, FLOOR), Rgb::BLACK);
        assert_ne!(palette::sample(Palette::Ember, 0.02, FLOOR), Rgb::BLACK);
    }

    #[test]
    fn the_ramp_clamps_out_of_range_input() {
        assert_eq!(palette::sample(Palette::Ember, -1.0, FLOOR), Rgb::BLACK);
        assert_eq!(
            palette::sample(Palette::Ember, 2.0, FLOOR),
            Rgb::new(255, 255, 255)
        );
    }

    #[test]
    fn the_default_look_is_the_v0_1_snake_and_ember() {
        let look = Look::default();
        assert_eq!(look.layout, Layout::Snake);
        assert_eq!(look.palette, Palette::Ember);
        let motion = MotionState::default().settings();
        assert_eq!(motion.motion, crate::motion::Motion::None);
        assert_eq!(motion.intensity, 1.0);
        assert_eq!(motion.speed, 1.0);
    }

    #[test]
    fn compose_honours_both_axes_of_the_look() {
        // Tiny rising band energies stay below the AGC gain ceiling, so they keep
        // distinct displayed levels. That makes either axis changing visible in
        // the frame.
        let mut bands = vec![0.0; BAND_COUNT];
        for (band, energy) in bands.iter_mut().enumerate() {
            *energy = 0.001 * (band + 1) as f32;
        }
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectrum_with(&bands), 1.0 / 30.0);

        let snake = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        let columns = compose(
            &dynamics,
            132,
            &Look {
                layout: Layout::Columns,
                palette: Palette::Ember,
            },
            &MotionState::default(),
        );
        let ice = compose(
            &dynamics,
            132,
            &Look {
                layout: Layout::Snake,
                palette: Palette::Ice,
            },
            &MotionState::default(),
        );
        assert_ne!(columns, snake, "changing the layout must move the bands");
        assert_ne!(ice, snake, "changing the palette must change the colours");

        // A layout is a permutation, so the same keys are lit either way.
        let lit = |frame: &[Rgb]| {
            frame
                .iter()
                .enumerate()
                .filter(|(_, colour)| **colour != Rgb::BLACK)
                .map(|(led, _)| led)
                .collect::<Vec<_>>()
        };
        assert_eq!(lit(&columns), lit(&snake));
    }

    #[test]
    fn a_composed_frame_lights_the_bands_and_thumbs_and_leaves_the_rest_dark() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 2, BAND_COUNT);
        let mut spectrum = Spectrum::new(2, BAND_COUNT);
        spectrum.set_bands(0, &[1.0; BAND_COUNT]);
        spectrum.set_bands(1, &[1.0; BAND_COUNT]);
        dynamics.update(&spectrum, 1.0 / 30.0);

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        let expected: Vec<u8> = [Half::Left, Half::Right]
            .into_iter()
            .flat_map(|half| {
                layout::band_leds(half)
                    .into_iter()
                    .chain(layout::thumb_leds(half).iter().copied())
            })
            .collect();
        for led in 0..132u8 {
            let should_be_lit = expected.contains(&led);
            assert_eq!(
                colours[led as usize] != Rgb::BLACK,
                should_be_lit,
                "LED {led} is {} when it should {}be lit",
                if should_be_lit { "dark" } else { "lit" },
                if should_be_lit { "" } else { "not " }
            );
        }
        // The rest of the underglow stays dark.
        let thumb_set: Vec<u8> = [Half::Left, Half::Right]
            .into_iter()
            .flat_map(layout::thumb_leds)
            .copied()
            .collect();
        for (led, colour) in colours.iter().enumerate().skip(69) {
            if thumb_set.contains(&(led as u8)) {
                continue;
            }
            assert_eq!(*colour, Rgb::BLACK, "underglow LED {led} must stay dark");
        }
    }

    #[test]
    fn a_higher_floor_blacks_a_decaying_band() {
        // The AGC normalises a sustained signal to the target (0.7), so a quiet
        // input alone stays lit. The floor matters during a fade: once the level
        // decays below it, the band goes black rather than glowing faintly.
        let config = DynamicsConfig {
            floor: 0.5,
            ..DynamicsConfig::default()
        };
        let mut dynamics = Dynamics::new(config, 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 0.5), 1.0 / 30.0);
        assert!(
            dynamics.level(0, 0) > 0.5,
            "a loud frame is above the floor"
        );

        // Fade until the level is below 0.5 but still above zero.
        for _ in 0..20 {
            dynamics.update(&spectral_flat(BAND_COUNT, 0.0), 1.0 / 30.0);
        }
        let level = dynamics.level(0, 0);
        assert!(
            level > 0.0 && level < 0.5,
            "the band is mid-fade, got {level}"
        );

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        for led in 0..132u8 {
            assert_eq!(
                colours[led as usize],
                Rgb::BLACK,
                "LED {led} must be black below the floor"
            );
        }
    }

    #[test]
    fn thumb_glow_disabled_keeps_thumbs_dark() {
        // With thumb_glow off — and the beat flash also off, so it cannot override —
        // the thumbs stay black even while the bands light.
        let config = DynamicsConfig {
            thumb_glow: false,
            beat_flash: false,
            ..DynamicsConfig::default()
        };
        let mut dynamics = Dynamics::new(config, 2, BAND_COUNT);
        let mut spectrum = Spectrum::new(2, BAND_COUNT);
        spectrum.set_bands(0, &[1.0; BAND_COUNT]);
        spectrum.set_bands(1, &[1.0; BAND_COUNT]);
        dynamics.update(&spectrum, 1.0 / 30.0);

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        for half in [Half::Left, Half::Right] {
            for led in layout::thumb_leds(half) {
                assert_eq!(
                    colours[*led as usize],
                    Rgb::BLACK,
                    "thumb {led} must be dark when thumb_glow is off"
                );
            }
        }
    }

    #[test]
    fn beat_flash_disabled_suppresses_the_white_flash() {
        // With beat_flash off but thumb_glow on, a beat leaves the thumbs at their
        // level colour rather than flashing white. The threshold is raised so the
        // beat detector stays quiet and only the glow branch runs.
        let config = DynamicsConfig {
            beat_flash: false,
            beat_threshold: 1_000.0,
            ..DynamicsConfig::default()
        };
        let mut dynamics = Dynamics::new(config, 1, BAND_COUNT);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        dynamics.update(&spectrum_with(&bands), 1.0 / 30.0);

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        for led in layout::thumb_leds(Half::Left) {
            assert_ne!(
                colours[*led as usize],
                Rgb::new(255, 255, 255),
                "thumb {led} must not flash white when beat_flash is off"
            );
            assert_ne!(
                colours[*led as usize],
                Rgb::BLACK,
                "thumb {led} must still show its level when thumb_glow is on"
            );
        }
    }

    #[test]
    fn thumb_clusters_reflect_their_own_channel() {
        // The AGC normalises every live channel to the same target, so differing
        // input levels would still produce the same colour. The honest test of
        // independence is one channel playing and the other silent: a shared
        // value would light both thumbs or neither.
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 2, BAND_COUNT);
        let mut bands_left = vec![0.0; BAND_COUNT];
        bands_left[..BASS_BANDS].fill(0.8);
        let mut spectrum = Spectrum::new(2, BAND_COUNT);
        spectrum.set_bands(0, &bands_left);
        spectrum.set_bands(1, &[0.0; BAND_COUNT]);

        // Let the flash end so the level colour is visible, not the white flash.
        for _ in 0..4 {
            dynamics.update(&spectrum, 1.0 / 30.0);
        }
        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        let left_thumb = layout::thumb_leds(Half::Left)[0];
        let right_thumb = layout::thumb_leds(Half::Right)[0];
        assert_ne!(
            colours[left_thumb as usize],
            Rgb::BLACK,
            "the playing channel's thumb must be lit"
        );
        assert_eq!(
            colours[right_thumb as usize],
            Rgb::BLACK,
            "the silent channel's thumb must be dark"
        );
    }

    #[test]
    fn ice_palette_colours_the_thumb_level_glow() {
        let config = DynamicsConfig {
            beat_flash: false,
            beat_threshold: 1_000.0,
            ..DynamicsConfig::default()
        };
        let mut dynamics = Dynamics::new(config, 2, BAND_COUNT);
        let mut bands_left = vec![0.0; BAND_COUNT];
        bands_left[..BASS_BANDS].fill(0.8);
        let mut spectrum = Spectrum::new(2, BAND_COUNT);
        spectrum.set_bands(0, &bands_left);
        spectrum.set_bands(1, &[0.0; BAND_COUNT]);
        dynamics.update(&spectrum, 1.0 / 30.0);

        let colours = compose(
            &dynamics,
            132,
            &Look {
                layout: Layout::Snake,
                palette: Palette::Ice,
            },
            &MotionState::default(),
        );
        for (half, channel) in [(Half::Left, 0), (Half::Right, 1)] {
            let expected =
                palette::sample(Palette::Ice, dynamics.thumb_level(channel), config.floor);
            for led in layout::thumb_leds(half) {
                assert_eq!(
                    colours[*led as usize],
                    expected,
                    "{} thumb {led} must use the active Ice palette",
                    half.name()
                );
            }
        }
        assert_ne!(
            colours[layout::thumb_leds(Half::Left)[0] as usize],
            Rgb::BLACK,
            "the playing channel's thumb should be lit"
        );
        assert_eq!(
            colours[layout::thumb_leds(Half::Right)[0] as usize],
            Rgb::BLACK,
            "the silent channel's thumb should be dark"
        );
    }

    #[test]
    fn ice_palette_does_not_change_the_white_beat_flash() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 0.0), 1.0 / 30.0);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        dynamics.update(&spectrum_with(&bands), 1.0 / 30.0);
        assert!(dynamics.is_flashing(0));

        let colours = compose(
            &dynamics,
            132,
            &Look {
                layout: Layout::Snake,
                palette: Palette::Ice,
            },
            &MotionState::default(),
        );
        for half in [Half::Left, Half::Right] {
            for led in layout::thumb_leds(half) {
                assert_eq!(
                    colours[*led as usize],
                    Rgb::new(255, 255, 255),
                    "{} thumb {led} must flash white regardless of palette",
                    half.name()
                );
            }
        }
    }

    #[test]
    fn matrix_bands_are_unaffected_by_the_beat_flash() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        let hit = spectrum_with(&bands);
        dynamics.update(&hit, 1.0 / 30.0);
        assert!(dynamics.is_flashing(0));

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        // The band LED for band 0 must be the ramp of the level, not white.
        let band_led = layout::band_leds(Half::Left)[0];
        assert_ne!(
            colours[band_led as usize],
            Rgb::new(255, 255, 255),
            "matrix bands must not flash"
        );
        // The thumb LED must be white.
        let thumb_led = layout::thumb_leds(Half::Left)[0];
        assert_eq!(
            colours[thumb_led as usize],
            Rgb::new(255, 255, 255),
            "the thumb must flash white"
        );
    }

    #[test]
    fn silence_leaves_the_thumbs_dark() {
        // A realistic noise floor, not exactly zero: below the gate but non-zero,
        // so an ungated beat detector would wrongly flash the thumbs. Exactly zero
        // would pass either way and hide the bug.
        let floor = 10f32.powf(-70.0 / 20.0);
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        for _ in 0..30 {
            dynamics.update(&spectral_flat(BAND_COUNT, floor), 1.0 / 30.0);
        }
        assert!(
            !dynamics.is_flashing(0),
            "the gate must stop a floor from flashing"
        );
        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        for half in [Half::Left, Half::Right] {
            for led in layout::thumb_leds(half) {
                assert_eq!(
                    colours[*led as usize],
                    Rgb::BLACK,
                    "thumb {led} must be dark in silence"
                );
            }
        }
    }

    #[test]
    fn a_stereo_capture_puts_the_left_channel_on_the_left_half() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 2, BAND_COUNT);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[0] = 0.9;
        let mut spectrum = Spectrum::new(2, BAND_COUNT);
        spectrum.set_bands(0, &bands);
        spectrum.set_bands(1, &[0.0; BAND_COUNT]);
        dynamics.update(&spectrum, 1.0 / 30.0);

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        let left_led = layout::band_leds(Half::Left)[0];
        let right_led = layout::band_leds(Half::Right)[0];
        assert_ne!(colours[left_led as usize], Rgb::BLACK);
        assert_eq!(colours[right_led as usize], Rgb::BLACK);
    }

    #[test]
    fn a_mono_capture_feeds_both_halves() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 0.5), 1.0 / 30.0);
        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        for half in [Half::Left, Half::Right] {
            let led = layout::band_leds(half)[0];
            assert_ne!(colours[led as usize], Rgb::BLACK);
        }
    }

    #[test]
    fn a_frame_smaller_than_the_grid_does_not_panic() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 1.0), 1.0 / 30.0);
        let colours = compose(&dynamics, 10, &Look::default(), &MotionState::default());
        assert_eq!(colours.len(), 10);
    }

    #[test]
    fn pacing_keeps_a_steady_period_without_drifting() {
        let start = Instant::now();
        let period = Duration::from_millis(33);
        let work = Duration::from_millis(10);
        let mut pacer = Pacer::new(period, start);

        let mut now = start;
        for frame in 0..100 {
            let (wait, resync) = pacer.wait(now);
            assert!(!resync);
            if frame > 0 {
                // The frame's own work is absorbed into the wait, so a frame that
                // takes 10 ms still starts on the same absolute grid.
                assert_eq!(wait, period - work, "frame {frame} did not absorb its work");
            }
            now += work + wait;
        }
        // Every frame but the last has its work absorbed by the following wait;
        // the last frame's work is simply what happens after the final deadline.
        assert_eq!(
            now - work,
            start + period * 100,
            "a hundred frames must land exactly on the hundredth deadline"
        );
    }

    #[test]
    fn a_stall_resynchronises_instead_of_writing_a_burst() {
        let start = Instant::now();
        let period = Duration::from_millis(20);
        let mut pacer = Pacer::new(period, start);

        // Ten periods pass while one frame is stuck.
        let (wait, resync) = pacer.wait(start + period * 10);
        assert!(resync, "a missed deadline must be reported");
        assert_eq!(wait, period, "the next frame is a full period away");
        assert_eq!(pacer.dropped(), 1);

        // And the schedule carries on from there rather than catching up.
        let (wait, resync) = pacer.wait(start + period * 11);
        assert!(!resync);
        assert_eq!(wait, Duration::ZERO);
        assert_eq!(pacer.dropped(), 1);
    }

    #[test]
    fn changing_the_rate_realigns_the_deadline_without_counting_a_miss() {
        let start = Instant::now();
        let mut pacer = Pacer::new(Duration::from_millis(33), start);

        // One frame at 30 fps, landing exactly on its deadline.
        let (wait, resync) = pacer.wait(start);
        assert!(!resync);
        assert_eq!(wait, Duration::from_millis(33));

        // The rate changes mid-run. The next deadline moves to one new period
        // from now, and a deliberate change must not be recorded as a stall.
        let now = start + Duration::from_millis(33);
        pacer.set_period(Duration::from_millis(66), now);
        let (wait, resync) = pacer.wait(now);
        assert!(!resync, "a deliberate rate change is not a missed deadline");
        assert_eq!(wait, Duration::from_millis(66));
        assert_eq!(pacer.dropped(), 0, "a rate change is not a dropped frame");
    }

    #[test]
    fn an_analyzer_drives_the_dynamics_end_to_end() {
        // The seam between the analyzer and the dynamics: real samples in, a
        // lit frame out, no hardware involved.
        let plan = BandPlan::standard(48_000, FFT_SIZE);
        let bands = plan.band_count();
        let mut analyzer = Analyzer::new(plan, 2);
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 2, bands);

        // A loud 1 kHz tone on both channels.
        let mut samples = Vec::new();
        for frame in 0..(FFT_SIZE * 2) {
            let value = (frame as f32 * 1_000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            samples.push(value);
            samples.push(value);
        }
        analyzer.push_interleaved(&samples);
        let spectrum = analyzer.spectrum().expect("a full window is ready");
        dynamics.update(spectrum, 1.0 / 30.0);

        let colours = compose(&dynamics, 132, &Look::default(), &MotionState::default());
        assert!(
            colours.iter().any(|colour| *colour != Rgb::BLACK),
            "a 1 kHz tone must light the board"
        );
    }
}
