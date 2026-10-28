//! Band dynamics and the composition of a spectrum into a frame.
//!
//! The spectrum from [`crate::spectrum`] is too raw to paint: transients last a
//! single frame, quiet masters barely move the LEDs, and digital silence still
//! carries a noise floor that per-band gain would happily amplify into a light
//! show. Three stages fix that, in this order:
//!
//! 1. **Gate** — if nothing on a channel is above `gate_db`, the channel is
//!    silent and goes black. The gate is measured on the loudest band, not on
//!    band 0: the lowest FFT bins are wider than the lowest bands, so a bass
//!    note can read a band or three high and band 0 can stay empty.
//! 2. **AGC** — each band's recent peak is tracked and scaled toward `target`.
//!    This is what lets a quietly mastered track fill the board, and why a loud
//!    one peaks at `target` rather than clipping every band to white.
//! 3. **Envelope** — instant attack, exponential decay. A transient is visible on
//!    the frame it arrives; everything else fades, so nothing strobes at the
//!    frame rate.
//!
//! All decay is expressed as `exp(-dt / tau)`, which makes the result independent
//! of the frame rate: the same music drawn at 15 or 60 fps decays at the same
//! speed in wall-clock time. A per-frame multiplier would strobe.

use std::time::{Duration, Instant};

use crate::layout::{self, Half, Layout};
use crate::level;
use crate::motion::MotionState;
use crate::palette::{self, Palette};
use crate::spectrum::Spectrum;
use crate::theme::Rgb;

/// How many low bands count as "bass" for beat detection.
///
/// The spec warns that band 0 alone is unreliable because the FFT bins are wider
/// than the lowest bands. Summing the lowest few bands covers the bass region
/// without trusting a single band.
pub const BASS_BANDS: usize = 4;

/// Tunables for [`Dynamics`]. Defaults come from the spec's settled decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicsConfig {
    /// Time constant of the fade after a transient. Short enough to follow the
    /// music, long enough that a 30 fps frame rate cannot strobe.
    pub decay_ms: f32,
    /// Time constant of the per-band peak tracker. Long, so the gain follows the
    /// track's mastering rather than every hit.
    pub agc_release_ms: f32,
    /// Below this, a channel is silent and painted black.
    pub gate_db: f32,
    /// What the AGC scales each band's peak to. Deliberately under 1.0 so a loud
    /// master lands on the top of the colour ramp without becoming white: the
    /// spec's ramp row (spec.md) keeps pure white for 1.0, which AGC never asks
    /// for.
    pub target: f32,
    /// Ceiling on AGC gain, so near-silence is not amplified without limit.
    /// 100 (40 dB) is chosen so a track mastered at the gate's -55 dBFS still
    /// reaches `target`; the gate rejects anything quieter before it matters.
    pub max_gain: f32,
    /// How much louder than the rolling average bass energy a beat must be to
    /// trigger a flash. The spec's default is 2.0.
    pub beat_threshold: f32,
    /// Time constant of the rolling average that the beat threshold compares
    /// against. Long enough that a single hit stands out, short enough that a
    /// sustained bass note raises the average and stops re-triggering.
    pub beat_release_ms: f32,
    /// How long the white flash lasts after a beat, in milliseconds.
    pub flash_ms: f32,
    /// Whether the thumb clusters glow with their channel's level.
    pub thumb_glow: bool,
    /// Whether the thumb clusters flash white on a beat.
    pub beat_flash: bool,
    /// Levels below this are drawn black, so silence goes dark. The spec's default
    /// is 0.02; configurable so a user can raise it on a dim board.
    pub floor: f32,
}

impl Default for DynamicsConfig {
    fn default() -> Self {
        Self {
            decay_ms: 180.0,
            agc_release_ms: 4_000.0,
            gate_db: -55.0,
            target: 0.7,
            max_gain: 100.0,
            beat_threshold: 2.0,
            beat_release_ms: 500.0,
            flash_ms: 80.0,
            thumb_glow: true,
            beat_flash: true,
            floor: FLOOR,
        }
    }
}

/// Per-band envelope follower with gating and automatic gain control.
#[derive(Debug, Clone)]
pub struct Dynamics {
    config: DynamicsConfig,
    channels: usize,
    bands: usize,
    /// Current displayed level per channel and band, 0..=1.
    levels: Vec<f32>,
    /// Tracked peak per channel and band, used for the gain.
    peaks: Vec<f32>,
    /// Whether each channel passed the gate on the last update.
    open: Vec<bool>,
    /// Rolling average of the bass energy per channel, for beat detection.
    energy_average: Vec<f32>,
    /// Whether the channel's bass energy was above the threshold last frame.
    was_above_threshold: Vec<bool>,
    /// Remaining time of the white flash per channel, in milliseconds.
    flash_remaining: Vec<f32>,
    /// The level shown on the thumb cluster per channel, 0..=1.
    thumb_level: Vec<f32>,
}

impl Dynamics {
    pub fn new(config: DynamicsConfig, channels: usize, bands: usize) -> Self {
        Self {
            config,
            channels,
            bands,
            levels: vec![0.0; channels * bands],
            peaks: vec![0.0; channels * bands],
            open: vec![false; channels],
            energy_average: vec![0.0; channels],
            was_above_threshold: vec![false; channels],
            flash_remaining: vec![0.0; channels],
            thumb_level: vec![0.0; channels],
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn bands(&self) -> usize {
        self.bands
    }

    #[cfg(test)]
    pub fn config(&self) -> DynamicsConfig {
        self.config
    }

    /// Whether a channel was above the gate on the last update.
    #[cfg(test)]
    pub fn is_open(&self, channel: usize) -> bool {
        self.open[channel]
    }

    /// The displayed level of one band, 0..=1.
    pub fn level(&self, channel: usize, band: usize) -> f32 {
        self.levels[channel * self.bands + band]
    }

    /// The level shown on one half's thumb cluster, 0..=1.
    pub fn thumb_level(&self, channel: usize) -> f32 {
        self.thumb_level[channel]
    }

    /// Whether a beat flash is currently active for this channel.
    pub fn is_flashing(&self, channel: usize) -> bool {
        self.flash_remaining[channel] > 0.0
    }

    /// Advances every band by `dt` seconds.
    pub fn update(&mut self, spectrum: &Spectrum, dt: f32) {
        debug_assert!(
            spectrum.channel_count() >= self.channels,
            "a {} channel spectrum cannot feed {} channels of dynamics",
            spectrum.channel_count(),
            self.channels
        );
        let dt = dt.max(0.0);
        let decay = (-dt * 1_000.0 / self.config.decay_ms.max(1.0)).exp();
        let release = (-dt * 1_000.0 / self.config.agc_release_ms.max(1.0)).exp();

        for channel in 0..self.channels {
            // The gate looks at the loudest band, so a bass note that reads high
            // in the band plan still proves the channel is alive.
            let loudest = spectrum.loudest_band(channel).1;
            let open = level::to_dbfs(loudest) >= self.config.gate_db;
            self.open[channel] = open;

            for (band, energy) in spectrum.bands(channel).iter().take(self.bands).enumerate() {
                let index = channel * self.bands + band;
                let raw = if open { *energy } else { 0.0 };

                // AGC: fast to raise the peak, slow to lower it.
                let peak = &mut self.peaks[index];
                *peak = if raw > *peak { raw } else { *peak * release };
                let gain = if *peak > 0.0 {
                    (self.config.target / *peak).min(self.config.max_gain)
                } else {
                    1.0
                };
                let scaled = (raw * gain).min(1.0);

                // Envelope: instant attack, exponential decay. Written as a max
                // rather than an if/else so that a steady input holds its level
                // instead of being decayed whenever the two happen to be equal.
                let level = &mut self.levels[index];
                *level = scaled.max(*level * decay);
            }

            // Beat detection: sum the lowest few bands, which cover the bass
            // region. Band 0 alone is unreliable because the FFT bins are wider
            // than the lowest bands (spec's ticket 03 note). A gated-closed
            // channel contributes nothing: its raw spectrum still carries a noise
            // floor, and letting that flash the thumbs would reintroduce for the
            // thumbs the "silence leaves the board dark" invariant this module
            // exists to keep.
            let bass_energy: f32 = if open {
                spectrum.bands(channel).iter().take(BASS_BANDS).sum()
            } else {
                0.0
            };
            let average = &mut self.energy_average[channel];
            let threshold = *average * self.config.beat_threshold;
            let above = bass_energy > threshold && bass_energy > 0.0;
            // Trigger only on a rising edge: a sustained tone would otherwise keep
            // re-triggering and leave the flash permanently on.
            if above && !self.was_above_threshold[channel] {
                self.flash_remaining[channel] = self.config.flash_ms;
            }
            self.was_above_threshold[channel] = above;

            // Decay the rolling average slowly, so a single hit stands out against
            // it and a sustained note quickly raises it.
            let beat_release = (-dt * 1_000.0 / self.config.beat_release_ms.max(1.0)).exp();
            *average = *average * beat_release + bass_energy * (1.0 - beat_release);

            // Decay the flash timer by the elapsed time.
            self.flash_remaining[channel] = (self.flash_remaining[channel] - dt * 1_000.0).max(0.0);

            // The thumb level is the loudest displayed bass band. The AGC and
            // envelope already normalised it, so it follows the music.
            let start = channel * self.bands;
            let end = start + BASS_BANDS.min(self.bands);
            self.thumb_level[channel] = self.levels[start..end]
                .iter()
                .copied()
                .fold(0.0f32, f32::max);
        }
    }
}

/// The default black floor. The config's `render.floor` can override it.
///
/// Levels below the floor are black. A decaying band approaches zero
/// asymptotically and never arrives, so without a floor a stalled stream would
/// leave a faint glow on the board forever instead of going dark — which is
/// exactly what the "silence leaves the board dark" criterion forbids.
pub const FLOOR: f32 = 0.02;

/// A fixed look: which layout places the bands and which palette colours them.
///
/// The default is the v0.1 look, `snake` + `ember`. Motion is deliberately not
/// stored here: it carries a clock and event state, so `compose` takes an
/// explicit `&MotionState` alongside the look. That is what lets a crossfade
/// render two selections in one frame without advancing events twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub layout: Layout,
    pub palette: Palette,
}

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
pub fn compose(dynamics: &Dynamics, count: usize, look: &Look, motion: &MotionState) -> Vec<Rgb> {
    let mut colours = vec![Rgb::BLACK; count];
    let config = dynamics.config;
    for (half, preferred) in [(Half::Left, 0), (Half::Right, 1)] {
        // A mono capture feeds both halves; a stereo one feeds its own side.
        let channel = if dynamics.channels() == 1 {
            0
        } else {
            preferred
        };
        for (band, (cell, led)) in layout::band_cells_for(&look.layout, half)
            .iter()
            .enumerate()
        {
            if band >= dynamics.bands() {
                break;
            }
            if let Some(colour) = colours.get_mut(*led as usize) {
                let sampled =
                    palette::sample(look.palette, dynamics.level(channel, band), config.floor);
                *colour = motion.modulate(sampled, *cell, preferred, channel, dynamics);
            }
        }

        // The thumb cluster carries the channel's level, and flashes white on a
        // beat before settling back. It is deliberately separate from the matrix
        // bands so the spectrum stays readable while the thumbs carry rhythm.
        let thumb_colour = if config.beat_flash && dynamics.is_flashing(channel) {
            Rgb::new(255, 255, 255)
        } else if config.thumb_glow {
            palette::sample(look.palette, dynamics.thumb_level(channel), config.floor)
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
    use crate::spectrum::{Analyzer, BandPlan, BAND_COUNT, FFT_SIZE};

    fn spectrum_with(values: &[f32]) -> Spectrum {
        let mut spectrum = Spectrum::new(1, values.len());
        spectrum.set_bands(0, values);
        spectrum
    }

    fn spectral_flat(bands: usize, energy: f32) -> Spectrum {
        spectrum_with(&vec![energy; bands])
    }

    #[test]
    fn a_transient_is_visible_on_the_frame_it_arrives() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 0.5), 1.0 / 30.0);
        // Instant attack: the first update already shows a substantial level.
        assert!(
            dynamics.level(0, 3) > 0.3,
            "attack should not ramp in, got {}",
            dynamics.level(0, 3)
        );
    }

    #[test]
    fn levels_fade_smoothly_and_monotonically_after_a_transient() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 0.5), 1.0 / 30.0);
        let first = dynamics.level(0, 5);
        let mut previous = first;
        for _ in 0..30 {
            dynamics.update(&spectral_flat(BAND_COUNT, 0.0), 1.0 / 30.0);
            let now = dynamics.level(0, 5);
            assert!(now < previous, "level must fall, {previous} -> {now}");
            assert!(
                now > previous * 0.5,
                "a 33 ms step must not drop the level by more than half, {previous} -> {now}"
            );
            previous = now;
        }
        assert!(previous < first * 0.1, "the fade should be complete in 1 s");
    }

    #[test]
    fn the_decay_does_not_depend_on_the_frame_rate() {
        // One 200 ms step and twenty 10 ms steps must land in the same place,
        // otherwise the picture stutters when the frame rate changes.
        let mut slow_decay = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        slow_decay.update(&spectral_flat(BAND_COUNT, 0.5), 0.01);
        slow_decay.update(&spectral_flat(BAND_COUNT, 0.0), 0.2);
        let mut fast_decay = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        fast_decay.update(&spectral_flat(BAND_COUNT, 0.5), 0.01);
        for _ in 0..20 {
            fast_decay.update(&spectral_flat(BAND_COUNT, 0.0), 0.01);
        }
        let slow_level = slow_decay.level(0, 2);
        let fast_level = fast_decay.level(0, 2);
        assert!(
            (slow_level - fast_level).abs() < 0.01,
            "one 200 ms step ({slow_level}) and twenty 10 ms steps ({fast_level}) must agree"
        );
    }

    #[test]
    fn silence_is_gated_to_black_even_though_the_agc_could_amplify_it() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        // A noise floor far below the gate.
        let floor = 10f32.powf(-70.0 / 20.0);
        for _ in 0..60 {
            dynamics.update(&spectral_flat(BAND_COUNT, floor), 1.0 / 30.0);
        }
        assert!(!dynamics.is_open(0));
        for band in 0..BAND_COUNT {
            assert_eq!(dynamics.level(0, band), 0.0, "band {band} leaked the floor");
        }
    }

    #[test]
    fn the_gate_opens_for_a_quiet_but_real_signal() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        // A quietly mastered track: peaks at -40 dB, well above the -55 dB gate.
        let quiet = 10f32.powf(-40.0 / 20.0);
        dynamics.update(&spectral_flat(BAND_COUNT, quiet), 1.0 / 30.0);
        assert!(dynamics.is_open(0));
        assert!(dynamics.level(0, 0) > 0.0);
    }

    #[test]
    fn the_gate_reads_the_loudest_band_not_band_zero() {
        // Bass reads high in the band plan, so band 0 can be empty while the
        // channel is loud. A gate tuned to band 0 would wrongly call this silent.
        let mut bands = vec![0.0; BAND_COUNT];
        bands[3] = 0.5;
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectrum_with(&bands), 1.0 / 30.0);
        assert!(dynamics.is_open(0));
        assert!(dynamics.level(0, 3) > 0.0);
    }

    #[test]
    fn a_quietly_mastered_track_is_lifted_toward_the_target() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        let quiet = 10f32.powf(-40.0 / 20.0);
        for _ in 0..90 {
            dynamics.update(&spectral_flat(BAND_COUNT, quiet), 1.0 / 30.0);
        }
        let level = dynamics.level(0, 4);
        assert!(
            level > 0.5,
            "a -40 dB master should still reach the board, got {level}"
        );
    }

    #[test]
    fn a_loud_master_does_not_become_a_wall_of_white() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        for _ in 0..90 {
            dynamics.update(&spectral_flat(BAND_COUNT, 1.0), 1.0 / 30.0);
        }
        for band in 0..BAND_COUNT {
            let level = dynamics.level(0, band);
            assert!(
                level <= dynamics.config().target + 0.01,
                "band {band} exceeded the AGC target: {level}"
            );
            assert_ne!(
                palette::sample(Palette::Ember, level, FLOOR),
                Rgb::new(255, 255, 255),
                "band {band} clipped to white"
            );
        }
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
    fn a_bass_beat_triggers_a_white_flash() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        // A silent frame first, so the rolling average starts at zero.
        dynamics.update(&spectral_flat(BAND_COUNT, 0.0), 1.0 / 30.0);
        assert!(!dynamics.is_flashing(0));

        // A bass hit: energy in the lowest BASS_BANDS bands.
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        dynamics.update(&spectrum_with(&bands), 1.0 / 30.0);
        assert!(dynamics.is_flashing(0), "a bass hit must trigger a flash");
    }

    #[test]
    fn a_sustained_bass_note_does_not_flash_forever() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        let hit = spectrum_with(&bands);

        // The first frame triggers a flash.
        dynamics.update(&hit, 1.0 / 30.0);
        assert!(dynamics.is_flashing(0));

        // After the flash duration (80 ms, ~3 frames), the flash ends.
        for _ in 0..3 {
            dynamics.update(&hit, 1.0 / 30.0);
        }
        assert!(
            !dynamics.is_flashing(0),
            "the flash must end after its duration"
        );

        // The sustained tone does not trigger a new flash (rising edge only).
        for _ in 0..30 {
            dynamics.update(&hit, 1.0 / 30.0);
        }
        assert!(
            !dynamics.is_flashing(0),
            "a sustained tone must not flash forever"
        );
    }

    #[test]
    fn the_flash_returns_to_the_level_colour() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        let hit = spectrum_with(&bands);

        dynamics.update(&hit, 1.0 / 30.0);
        assert!(dynamics.is_flashing(0));

        // After the flash ends, the thumb level should still be non-zero.
        for _ in 0..3 {
            dynamics.update(&hit, 1.0 / 30.0);
        }
        assert!(!dynamics.is_flashing(0));
        assert!(
            dynamics.thumb_level(0) > 0.0,
            "the level should still be visible after the flash"
        );
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
