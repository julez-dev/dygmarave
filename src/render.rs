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

use crate::layout::{self, Half};
use crate::level;
use crate::spectrum::Spectrum;
use crate::theme::Rgb;

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
}

impl Default for DynamicsConfig {
    fn default() -> Self {
        Self {
            decay_ms: 180.0,
            agc_release_ms: 4_000.0,
            gate_db: -55.0,
            target: 0.7,
            max_gain: 100.0,
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
        }
    }
}

/// Maps a level to a colour: black, through red and orange, to white.
///
/// The ramp reaches pure white only at 1.0, which the AGC never asks for, so a
/// loud track stays orange rather than becoming a wall of white.
///
/// Levels below [`ramp::FLOOR`] are black. A decaying band approaches zero
/// asymptotically and never arrives, so without a floor a stalled stream would
/// leave a faint red glow on the board forever instead of going dark — which is
/// exactly what the "silence leaves the board dark" criterion forbids.
pub fn ramp(level: f32) -> Rgb {
    const BLACK: Rgb = Rgb::BLACK;
    const RED: Rgb = Rgb::new(255, 0, 0);
    const ORANGE: Rgb = Rgb::new(255, 128, 0);
    const WHITE: Rgb = Rgb::new(255, 255, 255);

    let level = level.clamp(0.0, 1.0);
    if level < 0.02 {
        return BLACK;
    }

    // Each segment carries its own start and width, so a boundary cannot drift
    // from the branch that chose it: returning them together makes the two
    // impossible to disagree.
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

/// Paints the dynamics onto the grid: band *n* of a half lights that half's
/// *n*-th LED, inner to outer. LEDs without a band — underglow, the thumbs, and
/// the top bands the half has no keys for — stay black.
pub fn compose(dynamics: &Dynamics, count: usize) -> Vec<Rgb> {
    let mut colours = vec![Rgb::BLACK; count];
    for (half, preferred) in [(Half::Left, 0), (Half::Right, 1)] {
        // A mono capture feeds both halves; a stereo one feeds its own side.
        let channel = if dynamics.channels() == 1 {
            0
        } else {
            preferred
        };
        for (band, led) in layout::band_leds(half).iter().enumerate() {
            if band >= dynamics.bands() {
                break;
            }
            if let Some(colour) = colours.get_mut(*led as usize) {
                *colour = ramp(dynamics.level(channel, band));
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
                ramp(level),
                Rgb::new(255, 255, 255),
                "band {band} clipped to white"
            );
        }
    }

    #[test]
    fn the_ramp_runs_from_black_through_warm_to_white() {
        assert_eq!(ramp(0.0), Rgb::BLACK);
        assert_eq!(ramp(1.0), Rgb::new(255, 255, 255));
        assert_eq!(ramp(0.4), Rgb::new(255, 21, 0));
        assert_eq!(ramp(0.7), Rgb::new(255, 146, 36));

        // Brightness never falls as the level rises, so the picture reads as a
        // single ramp rather than reversing part way up.
        let mut previous = 0u32;
        for step in 0..=100 {
            let colour = ramp(step as f32 / 100.0);
            let brightness = colour.r as u32 + colour.g as u32 + colour.b as u32;
            assert!(brightness >= previous, "ramp reversed at {step}%");
            previous = brightness;
        }
    }

    #[test]
    fn the_ramp_floors_a_decaying_level_to_black() {
        // A fade approaches zero without arriving; without the floor the board
        // would glow faintly red forever after the music stops.
        assert_eq!(ramp(0.0), Rgb::BLACK);
        assert_eq!(ramp(0.019), Rgb::BLACK);
        assert_ne!(ramp(0.02), Rgb::BLACK);
    }

    #[test]
    fn the_ramp_clamps_out_of_range_input() {
        assert_eq!(ramp(-1.0), Rgb::BLACK);
        assert_eq!(ramp(2.0), Rgb::new(255, 255, 255));
    }

    #[test]
    fn a_composed_frame_lights_only_the_letter_keys_it_has_bands_for() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 2, BAND_COUNT);
        let mut spectrum = Spectrum::new(2, BAND_COUNT);
        spectrum.set_bands(0, &[1.0; BAND_COUNT]);
        spectrum.set_bands(1, &[1.0; BAND_COUNT]);
        dynamics.update(&spectrum, 1.0 / 30.0);

        let colours = compose(&dynamics, 132);
        let expected: Vec<u8> = [Half::Left, Half::Right]
            .into_iter()
            .flat_map(layout::band_leds)
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
        // Underglow and the thumbs stay dark.
        assert!(colours[69..].iter().all(|colour| *colour == Rgb::BLACK));
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

        let colours = compose(&dynamics, 132);
        let left_led = layout::band_leds(Half::Left)[0];
        let right_led = layout::band_leds(Half::Right)[0];
        assert_ne!(colours[left_led as usize], Rgb::BLACK);
        assert_eq!(colours[right_led as usize], Rgb::BLACK);
    }

    #[test]
    fn a_mono_capture_feeds_both_halves() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 0.5), 1.0 / 30.0);
        let colours = compose(&dynamics, 132);
        for half in [Half::Left, Half::Right] {
            let led = layout::band_leds(half)[0];
            assert_ne!(colours[led as usize], Rgb::BLACK);
        }
    }

    #[test]
    fn a_frame_smaller_than_the_grid_does_not_panic() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectral_flat(BAND_COUNT, 1.0), 1.0 / 30.0);
        let colours = compose(&dynamics, 10);
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

        let colours = compose(&dynamics, 132);
        assert!(
            colours.iter().any(|colour| *colour != Rgb::BLACK),
            "a 1 kHz tone must light the board"
        );
    }
}
