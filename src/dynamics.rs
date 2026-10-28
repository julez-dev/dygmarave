//! Band dynamics: gating, automatic gain control and the beat counter.
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

use crate::level;
use crate::spectrum::Spectrum;

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
    /// Monotonic count of rising-edge beats per channel. Motion events use this
    /// rather than the flash timer, which can expire between low-frame-rate
    /// advances and lasts several frames at high frame rates.
    beat_count: Vec<u64>,
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
            beat_count: vec![0; channels],
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

    pub(crate) fn config(&self) -> DynamicsConfig {
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
    ///
    /// This drives the white thumb flash only. Event motions must use
    /// [`Dynamics::beat_count`] instead: the flash can decay to zero before a
    /// low-frame-rate advance observes it, and stays true for several frames at
    /// higher rates.
    pub fn is_flashing(&self, channel: usize) -> bool {
        self.flash_remaining[channel] > 0.0
    }

    /// How many rising-edge beats this channel has seen, monotonically.
    pub fn beat_count(&self, channel: usize) -> u64 {
        self.beat_count[channel]
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
                self.beat_count[channel] += 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{self, Palette};
    use crate::spectrum::BAND_COUNT;
    use crate::theme::Rgb;

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
    fn the_beat_counter_counts_rising_edges_not_flash_frames() {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        let hit = spectrum_with(&bands);
        let silence = spectral_flat(BAND_COUNT, 0.0);

        dynamics.update(&silence, 0.2);
        assert_eq!(dynamics.beat_count(0), 0);
        dynamics.update(&hit, 0.2);
        assert_eq!(dynamics.beat_count(0), 1);
        assert!(
            !dynamics.is_flashing(0),
            "at 5 fps the flash has already expired; the counter must not"
        );

        // A sustained hit is not a new rising edge.
        dynamics.update(&hit, 0.2);
        assert_eq!(dynamics.beat_count(0), 1);

        dynamics.update(&silence, 0.2);
        dynamics.update(&hit, 0.2);
        assert_eq!(dynamics.beat_count(0), 2);
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
}
