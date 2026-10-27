//! The live visualizer: audio in, frames out, at a steady pace.
//!
//! This module is the product. It ties the three seams built earlier together —
//! capture (ticket 02), spectrum (ticket 03), grid and colour ramp (04, 05) —
//! and adds the one thing none of them owns: **what happens when the producer
//! outruns the consumer**.
//!
//! The capture callback runs on the audio thread and pushes blocks as fast as
//! the sound card delivers them. A frame takes far longer to write to the board
//! than a block takes to arrive, so a queue between them would grow without
//! bound and the picture would fall further behind the music every second. Each
//! frame therefore **drains the queue completely**, which keeps the backlog at
//! roughly one frame's worth of audio.
//!
//! Draining must not *skip* blocks, though. The analyzer's window is only the
//! newest 1024 samples, and it fills that window by appending what it is given.
//! Keeping the newest block and throwing away the ones in between would splice
//! audio from two different moments together — a step discontinuity whose
//! broadband click reads as energy in every band and makes the picture flash at
//! the frame rate. So all drained blocks are pushed in order and the analyzer's
//! own truncation keeps the window contiguous. Dropping only happens as a stall
//! safety valve, when the queue is far larger than a frame's worth.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::audio::Block;
use crate::render::{self, Dynamics, DynamicsConfig, Pacer};
use crate::spectrum::{Analyzer, BandPlan, Spectrum};
use crate::theme::Frame;

/// How the live loop is configured. The defaults are the spec's settled choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveConfig {
    /// Frames per second to target. 30 is smooth to the eye and leaves the wire
    /// time to absorb a slow frame without a visible stall.
    pub fps: u32,
    /// How many captured channels the board's two halves get fed from.
    pub channels: usize,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            channels: 2,
        }
    }
}

impl LiveConfig {
    pub fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps.max(1) as f64)
    }
}

/// The live loop's state: one analyzer, one set of dynamics, one pacer.
///
/// Kept separate from the capture and the link so the whole pipeline can be
/// driven in a test without a sound card or a keyboard.
pub struct Live {
    config: LiveConfig,
    analyzer: Analyzer,
    dynamics: Dynamics,
    pacer: Pacer,
    count: usize,
    /// Blocks discarded because a stall let the queue grow past the cap.
    dropped_blocks: u64,
    /// A zeroed spectrum, used to close the gate when audio stops arriving.
    silent: Spectrum,
    /// When audio last actually arrived. A suspended sink delivers no buffers at
    /// all and reports no error, so this is the only way to notice it.
    last_audio: Instant,
    /// When the previous frame was produced, for a measured decay interval.
    last_step: Instant,
    /// Frames that overran the schedule and had to resynchronise.
    resyncs: u64,
}

impl Live {
    pub fn new(config: LiveConfig, plan: BandPlan, count: usize, start: Instant) -> Self {
        let bands = plan.band_count();
        let dynamics_config = DynamicsConfig::default();
        Self {
            config,
            analyzer: Analyzer::new(plan, config.channels),
            dynamics: Dynamics::new(dynamics_config, config.channels, bands),
            pacer: Pacer::new(config.period(), start),
            count,
            dropped_blocks: 0,
            silent: Spectrum::new(config.channels, bands),
            last_audio: start,
            last_step: start,
            resyncs: 0,
        }
    }

    /// How many blocks were discarded because a frame was still being written.
    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks
    }

    /// Advances one frame and returns it.
    ///
    /// Pacing is *not* applied here: the caller writes the frame first and then
    /// asks [`Live::wait`] how long to sleep, so the board's write time is
    /// absorbed by the schedule instead of pushing every frame late.
    pub fn step(&mut self, latest: Option<&Block>, now: Instant, discarded: u64) -> Frame {
        self.dropped_blocks += discarded;

        // Measure the interval that actually elapsed rather than assuming the
        // nominal period. After a stall or a slow frame the fade must still take
        // decay_ms of wall-clock time, which is what makes it frame-rate
        // independent; a nominal dt would stretch the fade proportionally.
        let period = self.config.period();
        let dt = now
            .saturating_duration_since(self.last_step)
            .as_secs_f32()
            .clamp(0.0, 2.0 * period.as_secs_f32());
        self.last_step = now;

        if let Some(block) = latest {
            self.analyzer.push_interleaved(&block.samples);
            self.last_audio = now;
            if let Some(spectrum) = self.analyzer.spectrum() {
                self.dynamics.update(spectrum, dt);
            }
        } else if now.saturating_duration_since(self.last_audio) > 2 * period {
            // No audio for more than two frames: the stream has stopped without
            // reporting an error. Re-analysing the retained window would re-render
            // the last spectrum forever and leave the board lit while nothing is
            // playing. A zeroed spectrum closes the gate, so the picture fades out.
            self.dynamics.update(&self.silent, dt);
        }

        let colours = render::compose(&self.dynamics, self.count);
        Frame::from_colors(&colours)
    }

    /// How long to wait before the next frame, and whether the schedule resynced.
    pub fn wait(&mut self, now: Instant) -> Duration {
        let (wait, resync) = self.pacer.wait(now);
        if resync {
            self.resyncs += 1;
        }
        wait
    }

    /// Frames that overran the schedule. A rising count means the board cannot
    /// keep up with the requested rate; ticket 07 owns adapting to it.
    pub fn resyncs(&self) -> u64 {
        self.resyncs
    }

    /// How many blocks may wait before the oldest are discarded.
    ///
    /// A frame's worth of audio is two or three blocks, so this only trips after
    /// a multi-second stall — a failure of the board, not of the audio. Keeping
    /// the cap means even then the queue cannot grow without bound.
    pub const BACKLOG_CAP: usize = 32;

    /// Drains the channel into one contiguous block of samples.
    ///
    /// Returns the samples and how many blocks were discarded because the queue
    /// exceeded [`Live::BACKLOG_CAP`]. Draining here rather than in the capture
    /// thread is deliberate: the audio callback must never block.
    pub fn drain(receiver: &Receiver<Block>) -> (Option<Block>, u64) {
        let mut blocks: Vec<Block> = Vec::new();
        while let Ok(block) = receiver.try_recv() {
            blocks.push(block);
        }
        let mut dropped = 0;
        if blocks.len() > Self::BACKLOG_CAP {
            dropped = (blocks.len() - Self::BACKLOG_CAP) as u64;
            blocks.drain(..dropped as usize);
        }
        if blocks.is_empty() {
            return (None, 0);
        }
        let mut samples = Vec::with_capacity(blocks.iter().map(|block| block.samples.len()).sum());
        for block in blocks {
            samples.extend_from_slice(&block.samples);
        }
        (Some(Block { samples }), dropped)
    }

    /// Frames per second actually configured, for reporting.
    pub fn fps(&self) -> u32 {
        self.config.fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 48_000;
    const FFT_SIZE: usize = 1024;

    fn plan() -> BandPlan {
        BandPlan::standard(SAMPLE_RATE, FFT_SIZE)
    }

    /// A sine block long enough to fill the analyzer's window.
    fn tone_block(amplitude: f32, frequency: f32, channels: usize) -> Block {
        let mut samples = Vec::with_capacity(FFT_SIZE * channels);
        for frame in 0..FFT_SIZE {
            let value = (frame as f32 * frequency * std::f32::consts::TAU / SAMPLE_RATE as f32)
                .sin()
                * amplitude;
            for _ in 0..channels {
                samples.push(value);
            }
        }
        Block { samples }
    }

    #[test]
    fn a_tone_lights_the_board_and_silence_does_not() {
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);

        let mut now = start;
        for _ in 0..40 {
            let frame = live.step(Some(&tone_block(0.5, 1_000.0, 2)), now, 0);
            let colours =
                crate::theme::parse_theme_reply(frame.body().strip_prefix("led.theme ").unwrap())
                    .unwrap();
            if colours
                .iter()
                .any(|colour| *colour != crate::theme::Rgb::BLACK)
            {
                // A tone produced light within the first second.
                return;
            }
            now += live.wait(now);
        }
        panic!("a 1 kHz tone never lit the board");
    }

    #[test]
    fn a_tone_fades_to_black_when_the_stream_stops() {
        // The sink suspended: no buffers arrive and no error is reported. Feeding
        // the retained window would keep the board lit forever, so a stall longer
        // than two frames must close the gate and let the picture fade out.
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);
        let mut now = start;
        for _ in 0..40 {
            now += live.wait(now);
            let _ = live.step(Some(&tone_block(0.5, 1_000.0, 2)), now, 0);
        }

        // No audio for the rest of a second.
        for _ in 0..30 {
            now += live.wait(now);
            let _ = live.step(None, now, 0);
        }
        let frame = live.step(None, now, 0);
        let colours =
            crate::theme::parse_theme_reply(frame.body().strip_prefix("led.theme ").unwrap())
                .unwrap();
        assert!(
            colours
                .iter()
                .all(|colour| *colour == crate::theme::Rgb::BLACK),
            "a stalled stream must leave the board dark, not frozen lit"
        );
    }

    #[test]
    fn draining_keeps_the_audio_contiguous_and_the_queue_empty() {
        // Three blocks arrive between frames. Skipping the middle one would splice
        // two moments together and click; all three must be kept, in order.
        let (sender, receiver) = std::sync::mpsc::channel();
        for value in [0.1, 0.2, 0.3] {
            sender
                .send(Block {
                    samples: vec![value; 4],
                })
                .unwrap();
        }
        let (block, dropped) = Live::drain(&receiver);
        let block = block.expect("three blocks arrived");
        assert_eq!(dropped, 0, "a frame's worth of audio must not be dropped");
        assert_eq!(block.samples.len(), 12, "all three blocks are kept");
        assert_eq!(block.samples[0], 0.1);
        assert_eq!(block.samples[11], 0.3, "order is preserved, so no splice");

        // The channel is empty: the next frame waits on new audio, not a backlog.
        let (again, dropped_again) = Live::drain(&receiver);
        assert!(again.is_none());
        assert_eq!(dropped_again, 0);
    }

    #[test]
    fn a_long_stall_discards_the_oldest_audio_rather_than_growing_without_bound() {
        // A board stall let the queue grow past the cap; the newest audio is what
        // matters, so the oldest blocks go and the count is reported.
        let (sender, receiver) = std::sync::mpsc::channel();
        for index in 0..(Live::BACKLOG_CAP + 5) {
            // A distinct value per block, so dropping the *oldest* is provable:
            // identical samples would pass even if the newest were dropped instead.
            sender
                .send(Block {
                    samples: vec![index as f32; 4],
                })
                .unwrap();
        }
        let (block, dropped) = Live::drain(&receiver);
        let block = block.expect("blocks arrived");
        assert_eq!(dropped, 5, "only the overflow beyond the cap was dropped");
        assert_eq!(
            block.samples.len(),
            Live::BACKLOG_CAP * 4,
            "bounded by the cap"
        );
        assert_eq!(
            block.samples[0], 5.0,
            "the survivor starts at the first block after the drop"
        );
    }

    #[test]
    fn the_first_frames_are_black_while_the_window_fills() {
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);
        // A short block cannot fill a 1024-sample window.
        let short = Block {
            samples: vec![0.5; 64],
        };
        let frame = live.step(Some(&short), start, 0);
        let colours =
            crate::theme::parse_theme_reply(frame.body().strip_prefix("led.theme ").unwrap())
                .unwrap();
        assert!(colours
            .iter()
            .all(|colour| *colour == crate::theme::Rgb::BLACK));
    }

    #[test]
    fn the_frame_is_the_size_of_the_board() {
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);
        let frame = live.step(None, start, 0);
        assert_eq!(frame.leds(), 132);
    }

    #[test]
    fn pacing_holds_the_configured_rate() {
        let start = Instant::now();
        let config = LiveConfig {
            fps: 30,
            channels: 2,
        };
        let mut live = Live::new(config, plan(), 132, start);
        let mut now = start;
        for _ in 0..30 {
            let _frame = live.step(None, now, 0);
            now += live.wait(now);
        }
        // Thirty frames at 30 fps is one second of deadlines, within the timer's
        // own resolution: a Pacer running at twice the period would fail the upper
        // bound, and one that never waits would fail the lower.
        let elapsed = now.duration_since(start);
        assert!(
            elapsed >= Duration::from_millis(990) && elapsed <= Duration::from_millis(1_020),
            "30 frames should span one second, got {elapsed:?}"
        );
    }

    #[test]
    fn the_board_gets_full_frames_not_band_slices() {
        // compose() must always paint the whole board: a partial frame would be
        // refused by Session::write_frame, and the visualizer would stall.
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);
        let frame = live.step(Some(&tone_block(0.5, 1_000.0, 2)), start, 0);
        assert_eq!(frame.leds(), 132);
        // One colour per LED: 132 LEDs means 132 triplets.
        let values = frame
            .body()
            .strip_prefix("led.theme ")
            .unwrap()
            .split_whitespace()
            .count();
        assert_eq!(values, 132 * 3);
    }
}
