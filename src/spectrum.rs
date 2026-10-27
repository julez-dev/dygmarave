//! Turning samples into per-channel band energies.
//!
//! A window of samples per channel is Hann-windowed, transformed, and folded into
//! log-spaced bands. The output is a plain [`Spectrum`] of amplitudes that knows
//! nothing about keyboards or terminals, so the renderer can be built and tested
//! without a device attached.
//!
//! Two details are load-bearing:
//!
//! * **Band edges are log-spaced**, so bass and treble each own a comparable
//!   number of bands. Linear spacing would spend 27 of the 28 bands above 6 kHz.
//! * **Bands are integrated by overlapping power, not by picking bins.** A
//!   1024-sample window at 48 kHz has 46.9 Hz bins, wider than the bass bands, so
//!   a nearest-bin rule would leave most low bands empty and starve the display.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

/// The analysis window, ~21 ms at 48 kHz. Short enough to follow transients.
pub const FFT_SIZE: usize = 1024;

/// Bands per half. Matches the 28 matrix keys of one Dygma Raise half.
pub const BAND_COUNT: usize = 28;

/// Lowest band edge. Below this is mostly rumble and DC leakage.
pub const LOW_HZ: f32 = 40.0;

/// Highest band edge, just under Nyquist at 48 kHz and above most content.
pub const HIGH_HZ: f32 = 16_000.0;

/// Equivalent noise bandwidth of a Hann window, in bins.
///
/// A tone's main lobe spans several bins, so adding their power over-reads by
/// exactly this factor (sqrt(1.5) in amplitude). Dividing by it makes a
/// full-scale sine read 1.0 instead of 1.22.
const HANN_POWER_CORRECTION: f32 = 1.5;

/// Band edges and the geometry needed to fold bins into them.
#[derive(Debug, Clone)]
pub struct BandPlan {
    edges: Vec<f32>,
    fft_size: usize,
    sample_rate: u32,
}

impl BandPlan {
    /// Builds `bands` log-spaced bands between `low_hz` and `high_hz`.
    pub fn new(sample_rate: u32, fft_size: usize, bands: usize, low_hz: f32, high_hz: f32) -> Self {
        assert!(bands > 0, "a plan needs at least one band");
        assert!(low_hz > 0.0 && high_hz > low_hz, "band range must ascend");
        let ratio = (high_hz / low_hz).powf(1.0 / bands as f32);
        let edges = (0..=bands)
            .map(|index| low_hz * ratio.powi(index as i32))
            .collect();
        Self {
            edges,
            fft_size,
            sample_rate,
        }
    }

    /// The project defaults: 28 bands, 40 Hz–16 kHz.
    pub fn standard(sample_rate: u32, fft_size: usize) -> Self {
        Self::new(sample_rate, fft_size, BAND_COUNT, LOW_HZ, HIGH_HZ)
    }

    /// Band edges, lowest first, `band_count() + 1` of them.
    pub fn edges(&self) -> &[f32] {
        &self.edges
    }

    pub fn band_count(&self) -> usize {
        self.edges.len() - 1
    }

    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Centre frequency of a band, for labelling.
    pub fn centre_hz(&self, band: usize) -> f32 {
        (self.edges[band] + self.edges[band + 1]) / 2.0
    }

    /// The band whose range contains `hz`, if any.
    ///
    /// Used as the oracle in tests; the renderer maps bands to keys, not to
    /// frequencies, so nothing in the binary needs it yet.
    #[cfg(test)]
    pub fn band_for_hz(&self, hz: f32) -> Option<usize> {
        (0..self.band_count()).find(|band| hz >= self.edges[*band] && hz < self.edges[band + 1])
    }

    /// Width of one FFT bin, in Hz.
    fn bin_hz(&self) -> f32 {
        self.sample_rate as f32 / self.fft_size as f32
    }

    /// Adds each bin's power to the bands its frequency range overlaps.
    ///
    /// Split proportionally, so a bin shared by three narrow bass bands
    /// contributes a third to each instead of lighting all three fully.
    fn fold(&self, bin_power: &[f32], out: &mut [f32]) {
        let bin_hz = self.bin_hz();
        for (bin, power) in bin_power.iter().enumerate() {
            let bin_lo = bin as f32 * bin_hz;
            let bin_hi = bin_lo + bin_hz;
            for (band, slot) in out.iter_mut().enumerate().take(self.band_count()) {
                let lo = self.edges[band];
                let hi = self.edges[band + 1];
                let overlap = bin_hi.min(hi) - bin_lo.max(lo);
                if overlap > 0.0 {
                    *slot += power * (overlap / bin_hz);
                }
            }
        }
    }
}

/// One frame of band amplitudes per channel.
///
/// Linear amplitudes, not decibels: `1.0` is a full-scale sine, and the dynamics
/// stage decides how to compress that into light.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    per_channel: Vec<Vec<f32>>,
}

impl Spectrum {
    pub fn new(channels: usize, bands: usize) -> Self {
        Self {
            per_channel: vec![vec![0.0; bands]; channels],
        }
    }

    pub fn channel_count(&self) -> usize {
        self.per_channel.len()
    }

    /// Bands per channel. The renderer iterates [`Spectrum::bands`] instead, so
    /// this stays a test-side convenience for now.
    #[cfg(test)]
    pub fn band_count(&self) -> usize {
        self.per_channel.first().map_or(0, Vec::len)
    }

    pub fn bands(&self, channel: usize) -> &[f32] {
        &self.per_channel[channel]
    }

    /// One band's amplitude. Tests index single bands; the renderer takes the
    /// whole slice.
    #[cfg(test)]
    pub fn band(&self, channel: usize, band: usize) -> f32 {
        self.per_channel[channel][band]
    }

    /// The band carrying the most energy in a channel, and its amplitude.
    pub fn loudest_band(&self, channel: usize) -> (usize, f32) {
        self.per_channel[channel]
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |best, (band, value)| {
                if *value > best.1 {
                    (band, *value)
                } else {
                    best
                }
            })
    }
}

/// Accumulates samples and computes a spectrum whenever a full window is ready.
pub struct Analyzer {
    plan: BandPlan,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// Sum of the window, used to undo its amplitude loss.
    coherent_gain: f32,
    /// The most recent `fft_size` samples of each channel.
    pending: Vec<Vec<f32>>,
    input: Vec<f32>,
    output: Vec<Complex32>,
}

impl Analyzer {
    pub fn new(plan: BandPlan, channels: usize) -> Self {
        let fft_size = plan.fft_size();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);

        // Hann: good sidelobe behaviour for a display, and cheap.
        let window: Vec<f32> = (0..fft_size)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * index as f32 / fft_size as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();
        let coherent_gain = window.iter().sum::<f32>() / fft_size as f32;

        Self {
            pending: vec![Vec::with_capacity(fft_size * 2); channels],
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            window,
            coherent_gain,
            plan,
            fft,
        }
    }

    pub fn plan(&self) -> &BandPlan {
        &self.plan
    }

    /// Whether enough samples have arrived for a spectrum.
    pub fn is_ready(&self) -> bool {
        self.pending
            .iter()
            .all(|channel| channel.len() >= self.plan.fft_size())
    }

    /// Appends interleaved samples, keeping only the most recent window.
    pub fn push_interleaved(&mut self, samples: &[f32]) {
        let channels = self.pending.len();
        if channels == 0 {
            return;
        }
        let fft_size = self.plan.fft_size();
        // Whole frames only: a partial trailing frame belongs to no channel.
        for frame in samples.chunks_exact(channels) {
            for (channel, sample) in frame.iter().enumerate() {
                self.pending[channel].push(*sample);
            }
        }
        for channel in &mut self.pending {
            if channel.len() > fft_size {
                let excess = channel.len() - fft_size;
                channel.drain(..excess);
            }
        }
    }

    /// Computes the spectrum of the most recent window, if one is ready.
    pub fn spectrum(&mut self) -> Option<Spectrum> {
        if !self.is_ready() {
            return None;
        }
        let bands = self.plan.band_count();
        let mut spectrum = Spectrum::new(self.pending.len(), bands);
        let bin_scale = 2.0 / (self.plan.fft_size() as f32 * self.coherent_gain);
        let mut power = vec![0.0f32; self.output.len()];

        for (channel_index, samples) in self.pending.iter().enumerate() {
            for (index, sample) in samples.iter().enumerate() {
                self.input[index] = sample * self.window[index];
            }
            if self.fft.process(&mut self.input, &mut self.output).is_err() {
                return None;
            }
            // A real sine of amplitude A peaks at A after this scaling.
            for (bin, value) in self.output.iter().enumerate() {
                let amplitude = value.norm() * bin_scale;
                power[bin] = amplitude * amplitude;
            }
            self.plan
                .fold(&power, &mut spectrum.per_channel[channel_index]);
            for band in &mut spectrum.per_channel[channel_index] {
                *band = (*band / HANN_POWER_CORRECTION).sqrt();
            }
        }
        Some(spectrum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 48_000;

    fn analyzer(channels: usize) -> Analyzer {
        Analyzer::new(BandPlan::standard(SAMPLE_RATE, FFT_SIZE), channels)
    }

    /// Runs a sine through the analyzer and returns its spectrum.
    fn spectrum_of(sine: &[f32], channels: usize) -> Spectrum {
        let mut analyzer = analyzer(channels);
        analyzer.push_interleaved(sine);
        analyzer.spectrum().expect("a full window was pushed")
    }

    fn sine(freq: f32, amplitude: f32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| {
                amplitude
                    * (2.0 * std::f32::consts::PI * freq * index as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    #[test]
    fn band_edges_are_log_spaced_across_the_audio_range() {
        let plan = BandPlan::standard(SAMPLE_RATE, FFT_SIZE);
        let edges = plan.edges();
        assert_eq!(plan.band_count(), BAND_COUNT);
        assert_eq!(edges.len(), BAND_COUNT + 1);
        assert!((edges[0] - LOW_HZ).abs() < 0.01);
        assert!((edges[BAND_COUNT] - HIGH_HZ).abs() < 1.0);

        // Every band is the same factor wider than the one below it.
        let ratios: Vec<f32> = edges.windows(2).map(|pair| pair[1] / pair[0]).collect();
        let first = ratios[0];
        for ratio in &ratios {
            assert!(
                (ratio - first).abs() < 1e-3,
                "log spacing broken: {ratio} vs {first}"
            );
        }
        // Bass and treble own comparable spans: the top band is far wider.
        assert!(edges[BAND_COUNT] - edges[BAND_COUNT - 1] > 100.0 * (edges[1] - edges[0]));
    }

    #[test]
    fn a_tone_lands_in_the_band_that_contains_its_frequency() {
        // Bands here are several bins wide, so the assignment is unambiguous.
        let plan = BandPlan::standard(SAMPLE_RATE, FFT_SIZE);
        for hz in [1_000.0, 4_000.0, 8_000.0, 12_000.0] {
            let spectrum = spectrum_of(&sine(hz, 0.5, FFT_SIZE * 2), 1);
            let (band, amplitude) = spectrum.loudest_band(0);
            assert_eq!(
                Some(band),
                plan.band_for_hz(hz),
                "{hz} Hz landed in band {band}"
            );
            // Scalloping loss costs up to ~15% when a tone sits between bins.
            assert!(
                (amplitude - 0.5).abs() < 0.1,
                "{hz} Hz should read ~0.5, got {amplitude}"
            );
        }
    }

    #[test]
    fn bass_is_coarser_than_one_band_per_hz_and_that_is_expected() {
        // 1024 samples at 48 kHz gives 46.9 Hz bins, wider than the lowest bands
        // (40-62 Hz), so a low tone's energy legitimately straddles neighbours.
        // The display only needs the bass region to move in the right direction.
        let plan = BandPlan::standard(SAMPLE_RATE, FFT_SIZE);
        for hz in [63.0, 100.0, 200.0] {
            let spectrum = spectrum_of(&sine(hz, 0.5, FFT_SIZE * 2), 1);
            let (band, amplitude) = spectrum.loudest_band(0);
            let expected = plan.band_for_hz(hz).expect("in range");
            assert!(
                band.abs_diff(expected) <= 1,
                "{hz} Hz landed in band {band}, expected near {expected}"
            );
            assert!(amplitude > 0.1, "{hz} Hz should still be visible");
        }
    }

    #[test]
    fn a_channel_without_a_tone_stays_silent() {
        // 1 kHz panned hard left.
        let mut interleaved = Vec::with_capacity(FFT_SIZE * 2);
        for sample in sine(1_000.0, 0.5, FFT_SIZE * 2) {
            interleaved.push(sample);
            interleaved.push(0.0);
        }
        let spectrum = spectrum_of(&interleaved, 2);
        assert!(
            spectrum.loudest_band(0).1 > 0.4,
            "left should hear the tone"
        );
        assert!(
            spectrum.bands(1).iter().all(|value| *value < 1e-3),
            "right must stay silent, got {:?}",
            spectrum.bands(1)
        );
    }

    #[test]
    fn a_sweep_moves_energy_from_low_bands_to_high_bands() {
        let mut previous = 0;
        for hz in [
            60.0, 120.0, 240.0, 480.0, 960.0, 1_920.0, 3_840.0, 7_680.0, 15_000.0,
        ] {
            let spectrum = spectrum_of(&sine(hz, 0.5, FFT_SIZE * 2), 1);
            let (band, _) = spectrum.loudest_band(0);
            assert!(
                band >= previous,
                "{hz} Hz went backwards: band {band} after {previous}"
            );
            previous = band;
        }
        assert_eq!(previous, BAND_COUNT - 1, "15 kHz should reach the top band");
    }

    #[test]
    fn silence_produces_no_energy() {
        let spectrum = spectrum_of(&[0.0; FFT_SIZE * 2], 1);
        assert!(spectrum.bands(0).iter().all(|value| *value == 0.0));
    }

    #[test]
    fn no_spectrum_until_a_full_window_arrives() {
        let mut analyzer = analyzer(1);
        analyzer.push_interleaved(&[0.1; FFT_SIZE / 2]);
        assert!(!analyzer.is_ready());
        assert!(analyzer.spectrum().is_none());
        analyzer.push_interleaved(&[0.1; FFT_SIZE / 2]);
        assert!(analyzer.is_ready());
        assert!(analyzer.spectrum().is_some());
    }

    #[test]
    fn keeps_only_the_most_recent_window() {
        let mut analyzer = analyzer(1);
        // More than a window of loud samples, then silence.
        analyzer.push_interleaved(&[0.5; FFT_SIZE * 3]);
        analyzer.push_interleaved(&[0.0; FFT_SIZE * 2]);
        let spectrum = analyzer.spectrum().unwrap();
        assert!(spectrum.bands(0).iter().all(|value| *value == 0.0));
    }

    #[test]
    fn a_view_of_the_spectrum_is_device_free() {
        // The type exposes only channels and bands: no keyboard concepts.
        let spectrum = spectrum_of(&sine(440.0, 0.5, FFT_SIZE * 2), 2);
        assert_eq!(spectrum.channel_count(), 2);
        assert_eq!(spectrum.band_count(), BAND_COUNT);
        assert_eq!(spectrum.band(0, 0), spectrum.bands(0)[0]);
    }
}
