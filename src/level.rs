//! Turning interleaved samples into numbers a person can read.

/// Peak and RMS per channel over one block of samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub peak: Vec<f32>,
    pub rms: Vec<f32>,
}

impl Levels {
    /// The loudest peak across all channels.
    pub fn loudest(&self) -> f32 {
        self.peak.iter().copied().fold(0.0, f32::max)
    }
}

/// Measures per-channel peak and RMS from interleaved samples.
///
/// Returns `None` for a block with no samples or no channels, rather than
/// inventing a silent reading.
pub fn measure(samples: &[f32], channels: u16) -> Option<Levels> {
    if samples.is_empty() || channels == 0 {
        return None;
    }
    let channels = channels as usize;
    let mut peak = vec![0.0f32; channels];
    let mut sum_squares = vec![0.0f32; channels];
    let mut counts = vec![0usize; channels];

    for (index, sample) in samples.iter().enumerate() {
        let channel = index % channels;
        let magnitude = sample.abs();
        if magnitude > peak[channel] {
            peak[channel] = magnitude;
        }
        sum_squares[channel] += sample * sample;
        counts[channel] += 1;
    }

    let rms = sum_squares
        .iter()
        .zip(counts.iter())
        .map(|(sum, count)| {
            if *count == 0 {
                0.0
            } else {
                (sum / *count as f32).sqrt()
            }
        })
        .collect();

    Some(Levels { peak, rms })
}

/// Converts a linear amplitude to dBFS, clamping silence to the floor.
pub fn to_dbfs(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return -100.0;
    }
    (20.0 * amplitude.log10()).max(-100.0)
}

/// Renders a dBFS value as a short bar, full scale at 0 dBFS and empty silence.
pub fn bar(dbfs: f32, width: usize) -> String {
    const STEPS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    // A 60 dB window is enough to see music without wasting the display on
    // inaudible detail.
    let proportion = ((dbfs + 60.0) / 60.0).clamp(0.0, 1.0);
    let filled = proportion * width as f32;
    let whole = filled.floor() as usize;
    let mut out = String::with_capacity(width);
    for cell in 0..width {
        if cell < whole {
            out.push(STEPS[8]);
        } else if cell == whole {
            let fraction = filled - whole as f32;
            let step = (fraction * 8.0).round() as usize;
            out.push(STEPS[step.min(8)]);
        } else {
            out.push(STEPS[0]);
        }
    }
    out
}

/// Renders all channels on one terminal line so it can be redrawn in place.
pub fn meter_line(levels: &Levels) -> String {
    let mut parts = Vec::with_capacity(levels.peak.len());
    for (index, peak) in levels.peak.iter().enumerate() {
        let label = channel_label(index, levels.peak.len());
        let dbfs = to_dbfs(*peak);
        parts.push(format!("{label} {} {:>6.1} dBFS", bar(dbfs, 12), dbfs));
    }
    parts.join("   ")
}

fn channel_label(index: usize, channels: usize) -> String {
    match (channels, index) {
        (1, _) => "M".to_string(),
        (2, 0) => "L".to_string(),
        (2, 1) => "R".to_string(),
        _ => format!("{}", index + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_each_channel_separately() {
        // Interleaved stereo: left loud, right silent.
        let samples = [0.5, 0.0, -0.5, 0.0];
        let levels = measure(&samples, 2).unwrap();
        assert_eq!(levels.peak, vec![0.5, 0.0]);
        assert!((levels.rms[0] - 0.5).abs() < 1e-6);
        assert_eq!(levels.rms[1], 0.0);
        assert_eq!(levels.loudest(), 0.5);
    }

    #[test]
    fn peak_uses_magnitude_not_sign() {
        let levels = measure(&[-1.0, 0.25], 1).unwrap();
        assert_eq!(levels.peak, vec![1.0]);
    }

    #[test]
    fn rms_of_a_sine_is_its_amplitude_over_root_two() {
        let samples: Vec<f32> = (0..1024)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI / 64.0).sin())
            .collect();
        let levels = measure(&samples, 1).unwrap();
        assert!((levels.rms[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn an_empty_block_has_no_levels() {
        assert!(measure(&[], 2).is_none());
        assert!(measure(&[0.1], 0).is_none());
    }

    #[test]
    fn converts_to_dbfs_with_a_floor() {
        assert!((to_dbfs(1.0) - 0.0).abs() < 1e-6);
        assert!((to_dbfs(0.5) + 6.02).abs() < 0.02);
        assert_eq!(to_dbfs(0.0), -100.0);
        assert_eq!(to_dbfs(-1.0), -100.0);
    }

    #[test]
    fn a_meter_line_names_each_channel_and_its_level() {
        let levels = Levels {
            peak: vec![1.0, 0.0],
            rms: vec![0.5, 0.0],
        };
        let line = meter_line(&levels);
        assert!(line.contains('L') && line.contains('R'));
        assert!(line.contains("0.0 dBFS"), "left is full scale: {line}");
        assert!(line.contains("-100.0 dBFS"), "right is silent: {line}");
    }

    #[test]
    fn a_mono_meter_line_is_labelled_m() {
        let levels = Levels {
            peak: vec![0.5],
            rms: vec![0.25],
        };
        assert!(meter_line(&levels).starts_with('M'));
    }

    #[test]
    fn bars_span_silence_to_full_scale() {
        assert_eq!(bar(-100.0, 4), "    ");
        assert_eq!(bar(0.0, 4), "████");
        // Half-scale is 6 dB down in a 60 dB window: still mostly filled.
        let half = bar(-6.0, 10);
        assert!(half.contains('█'));
        assert_eq!(half.chars().count(), 10);
    }
}
