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

/// The period of a frame rate, never dividing by zero.
fn period_of(fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / fps.max(1) as f64)
}

/// Tunables for adaptive pacing.
///
/// The spec settles the 30 fps target and that the rate adapts downward; the rest
/// — a 5 fps floor, two resyncs in a two-second window counted as "cannot keep
/// up", and six clean seconds before stepping back up — are this module's choices,
/// recorded in ticket 07.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateConfig {
    /// The rate to aim for when the board keeps up.
    pub target: u32,
    /// The lowest rate the controller will fall to. A slow board gets a choppier
    /// picture, never a coarser one: the band plan and FFT window never change.
    pub min: u32,
    /// How many pacing resyncs within one window mean the board cannot keep up.
    pub degrade_after: u32,
    /// How long one observation window lasts.
    pub window: Duration,
    /// How long the board must run clean before the rate steps back up.
    pub recover_after: Duration,
}

impl Default for RateConfig {
    fn default() -> Self {
        Self {
            target: 30,
            min: 5,
            degrade_after: 2,
            window: Duration::from_secs(2),
            recover_after: Duration::from_secs(6),
        }
    }
}

/// Adaptive frame-rate control.
///
/// The board's write time is not constant: a slow USB hub, a busy CPU or a
/// firmware hiccup can make a frame take longer than the target period. When that
/// happens the honest response is to write *fewer* frames — the picture gets
/// choppier, but every band keeps its full resolution — and to step back up when
/// the board recovers. Only the frame rate moves; the band plan, FFT window and
/// band count are never touched.
///
/// The controller observes one window at a time. A window in which the pacer had
/// to resynchronise [`RateConfig::degrade_after`] times, or in which fewer than
/// 90% of the current rate's frames arrived, is a window the board could not keep
/// up with, and the rate steps down by a sixth of the target. A window clean for
/// [`RateConfig::recover_after`] steps back up by the same amount. This is the
/// usual additive-increase/multiplicative-decrease loop: it settles just below
/// the rate the board can actually hold and probes upward periodically.
pub struct RateControl {
    config: RateConfig,
    current: u32,
    window_resyncs: u32,
    window_frames: u64,
    window_start: Instant,
    changed_at: Instant,
    /// The frame rate achieved over the last closed window, if any.
    recent: Option<f64>,
}

impl RateControl {
    pub fn new(config: RateConfig, start: Instant) -> Self {
        // A floor above the target would make "degrading" raise the rate — reachable
        // with `--fps 1..4`, where the default 5 fps floor exceeds the target — so
        // clamp it to the target. The window is assumed non-zero by construction.
        let config = RateConfig {
            target: config.target.max(1),
            min: config.min.min(config.target).max(1),
            ..config
        };
        Self {
            config,
            current: config.target,
            window_resyncs: 0,
            window_frames: 0,
            window_start: start,
            changed_at: start,
            recent: None,
        }
    }

    /// The rate currently being targeted.
    pub fn fps(&self) -> u32 {
        self.current
    }

    /// The rate to aim for when the board keeps up.
    pub fn target(&self) -> u32 {
        self.config.target
    }

    /// The frame rate achieved over the last closed observation window. `None`
    /// until the first window closes. Unlike a cumulative average, this stays
    /// honest when an hour of healthy running hides a recent slowdown.
    pub fn recent_fps(&self) -> Option<f64> {
        self.recent
    }

    /// Records one frame's outcome and, when the window closes, returns the new
    /// rate if it changed.
    pub fn observe(&mut self, now: Instant, resync: bool) -> Option<u32> {
        self.window_frames += 1;
        if resync {
            self.window_resyncs += 1;
        }
        let elapsed = now.saturating_duration_since(self.window_start);
        if elapsed < self.config.window {
            return None;
        }

        // Two independent signs the board could not keep up: the pacer had to
        // give up a deadline repeatedly, or the frames that did arrive were far
        // fewer than the current rate asks for.
        let achieved = self.window_frames as f64 / elapsed.as_secs_f64();
        let too_slow = self.window_resyncs >= self.config.degrade_after
            || achieved < self.current as f64 * 0.9;
        let step = (self.config.target / 6).max(1);
        let previous = self.current;

        if too_slow {
            let next = self.current.saturating_sub(step).max(self.config.min);
            if next != self.current {
                self.current = next;
                self.changed_at = now;
            }
        } else if self.current < self.config.target
            && now.saturating_duration_since(self.changed_at) >= self.config.recover_after
        {
            self.current = (self.current + step).min(self.config.target);
            self.changed_at = now;
        }

        self.recent = Some(achieved);
        self.window_resyncs = 0;
        self.window_frames = 0;
        self.window_start = now;

        (self.current != previous).then_some(self.current)
    }
}

/// The live loop's state: one analyzer, one set of dynamics, one pacer.
///
/// Kept separate from the capture and the link so the whole pipeline can be
/// driven in a test without a sound card or a keyboard.
pub struct Live {
    analyzer: Analyzer,
    dynamics: Dynamics,
    pacer: Pacer,
    /// Adaptive frame-rate control. Only the rate moves; spectral resolution is
    /// never reduced.
    rate: RateControl,
    /// How many LEDs the board has.
    leds: usize,
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
    /// Frames produced so far, for the achieved-rate report.
    frames: u64,
    /// When this visualizer started, for the achieved-rate report.
    started: Instant,
}

impl Live {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(config: LiveConfig, plan: BandPlan, count: usize, start: Instant) -> Self {
        Self::with_dynamics(config, DynamicsConfig::default(), plan, count, start)
    }

    /// Creates a visualizer with explicit dynamics settings, so the config file's
    /// render and thumb sections take effect without a rebuild.
    pub fn with_dynamics(
        config: LiveConfig,
        dynamics_config: DynamicsConfig,
        plan: BandPlan,
        count: usize,
        start: Instant,
    ) -> Self {
        let bands = plan.band_count();
        Self {
            analyzer: Analyzer::new(plan, config.channels),
            dynamics: Dynamics::new(dynamics_config, config.channels, bands),
            pacer: Pacer::new(period_of(config.fps), start),
            rate: RateControl::new(
                RateConfig {
                    target: config.fps,
                    ..RateConfig::default()
                },
                start,
            ),
            leds: count,
            dropped_blocks: 0,
            silent: Spectrum::new(config.channels, bands),
            last_audio: start,
            last_step: start,
            resyncs: 0,
            frames: 0,
            started: start,
        }
    }

    /// The period of the rate currently being targeted.
    fn period(&self) -> Duration {
        period_of(self.rate.fps())
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
        self.frames += 1;

        // Measure the interval that actually elapsed rather than assuming the
        // nominal period. After a stall or a slow frame the fade must still take
        // decay_ms of wall-clock time, which is what makes it frame-rate
        // independent; a nominal dt would stretch the fade proportionally.
        let period = self.period();
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

        let colours = render::compose(&self.dynamics, self.leds, &render::Look::default());
        Frame::from_colors(&colours)
    }

    /// How long to wait before the next frame.
    ///
    /// When the board cannot keep up this also lowers the rate, and after a clean
    /// spell it raises it again. The returned duration is the new period when the
    /// rate changed, so the caller sleeps the new rate immediately.
    pub fn wait(&mut self, now: Instant) -> Duration {
        let (wait, resync) = self.pacer.wait(now);
        if resync {
            self.resyncs += 1;
        }
        match self.rate.observe(now, resync) {
            Some(fps) => {
                let period = period_of(fps);
                self.pacer.set_period(period, now);
                period
            }
            None => wait,
        }
    }

    /// Frames that overran the schedule. A rising count means the board could not
    /// keep up; [`RateControl`] responds by lowering the rate.
    pub fn resyncs(&self) -> u64 {
        self.resyncs
    }

    /// The rate to aim for when the board keeps up.
    pub fn target_fps(&self) -> u32 {
        self.rate.target()
    }

    /// Frames produced so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The rate achieved over the most recent observation window, when one has
    /// closed. Unlike [`Live::achieved_fps`], this is not averaged over the whole
    /// run, so a recent slowdown stays visible.
    pub fn recent_fps(&self) -> Option<f64> {
        self.rate.recent_fps()
    }

    /// The rate actually being achieved, averaged since this visualizer started.
    pub fn achieved_fps(&self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f64();
        if elapsed <= 0.0 {
            return 0.0;
        }
        self.frames as f64 / elapsed
    }

    /// How many spectral bands each channel is analysed into. Constant at every
    /// frame rate — a slower board gets a choppier picture, never a coarser one.
    pub fn bands(&self) -> usize {
        self.analyzer.plan().band_count()
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

    /// Frames per second currently being targeted, for reporting. This falls when
    /// the board cannot keep up and rises when it recovers.
    pub fn fps(&self) -> u32 {
        self.rate.fps()
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

    #[test]
    fn rate_control_starts_at_the_target() {
        let start = Instant::now();
        let control = RateControl::new(RateConfig::default(), start);
        assert_eq!(control.fps(), 30);
        assert_eq!(control.target(), 30);
        assert_eq!(RateConfig::default().min, 5);
    }

    #[test]
    fn a_window_with_repeated_resyncs_lowers_the_rate() {
        let start = Instant::now();
        let mut control = RateControl::new(RateConfig::default(), start);
        // Two resyncs inside the two-second window: the board cannot keep up.
        assert_eq!(
            control.observe(start + Duration::from_millis(500), true),
            None
        );
        assert_eq!(
            control.observe(start + Duration::from_millis(1_000), true),
            None
        );
        let changed = control.observe(start + Duration::from_millis(2_000), true);
        assert_eq!(
            changed,
            Some(25),
            "the rate steps down by a sixth of the target"
        );
    }

    #[test]
    fn a_sustained_shortfall_lowers_the_rate_without_any_resync() {
        let start = Instant::now();
        let mut control = RateControl::new(RateConfig::default(), start);
        // The pacer was never late, but only two frames arrived in two seconds.
        assert_eq!(control.recent_fps(), None, "no window has closed yet");
        assert_eq!(control.observe(start + Duration::from_secs(1), false), None);
        let changed = control.observe(start + Duration::from_secs(2), false);
        assert_eq!(changed, Some(25), "one frame per second cannot hold 30 fps");
        let recent = control.recent_fps().expect("the window closed");
        assert!(
            (0.9..=1.1).contains(&recent),
            "the window's achieved rate is reported, got {recent}"
        );
    }

    #[test]
    fn a_healthy_window_holds_the_rate() {
        let start = Instant::now();
        let mut control = RateControl::new(RateConfig::default(), start);
        // Sixty frames in under two seconds is the target rate, with no resyncs.
        for frame in 1..=60 {
            let now = start + Duration::from_millis(frame * 33);
            assert_eq!(
                control.observe(now, false),
                None,
                "a healthy window must not change the rate"
            );
        }
        assert_eq!(control.fps(), 30);
    }

    #[test]
    fn a_clean_spell_raises_the_rate_again() {
        let start = Instant::now();
        let mut control = RateControl::new(RateConfig::default(), start);
        // Degrade first: two resyncs in the first window.
        control.observe(start + Duration::from_millis(1_000), true);
        assert_eq!(
            control.observe(start + Duration::from_millis(2_000), true),
            Some(25)
        );

        // Then run clean at 25 fps for longer than the recovery delay. Windows
        // close every two seconds; the first step up needs six seconds since the
        // change, so it lands at the eighth second.
        let mut now = start + Duration::from_millis(2_000);
        let mut raised = None;
        while now < start + Duration::from_secs(12) {
            now += Duration::from_millis(40); // 25 fps: healthy against 25
            if let Some(fps) = control.observe(now, false) {
                raised = Some(fps);
                break;
            }
        }
        assert_eq!(
            raised,
            Some(30),
            "a clean spell must step back toward the target"
        );
    }

    #[test]
    fn the_rate_stays_between_the_floor_and_the_target() {
        let start = Instant::now();
        let mut control = RateControl::new(RateConfig::default(), start);
        let mut now = start;
        let floor = RateConfig::default().min;

        // Twenty windows of total failure: the rate bottoms out at the floor.
        for _ in 0..20 {
            now += Duration::from_secs(2);
            control.observe(now, true);
            assert!(control.fps() >= floor, "the floor must hold");
            assert!(control.fps() <= control.target());
        }
        assert_eq!(control.fps(), floor);

        // Then a minute of clean running: it recovers to the target and stops.
        let end = now + Duration::from_secs(60);
        while now < end {
            now += Duration::from_secs_f64(1.0 / control.fps() as f64);
            control.observe(now, false);
            assert!(control.fps() >= floor);
            assert!(control.fps() <= control.target());
        }
        assert_eq!(control.fps(), control.target());
    }

    #[test]
    fn a_lowered_rate_still_delivers_the_full_spectrum() {
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);
        let bands = live.bands();

        // Stall past the deadline repeatedly, so each wait resynchronises.
        let mut now = start;
        for _ in 0..4 {
            now += Duration::from_secs(1);
            let _ = live.wait(now);
        }
        assert!(
            live.fps() < live.target_fps(),
            "a slow board must lower the rate"
        );
        // Spectral fidelity is never the thing that gives: the band plan is intact
        // and the frame still covers the whole board.
        assert_eq!(live.bands(), bands);
        assert_eq!(live.step(None, now, 0).leds(), 132);
    }

    #[test]
    fn the_achieved_rate_is_reported_against_the_target() {
        let start = Instant::now();
        let mut live = Live::new(LiveConfig::default(), plan(), 132, start);
        let mut now = start;
        for _ in 0..30 {
            now += live.wait(now);
            let _ = live.step(None, now, 0);
        }
        let achieved = live.achieved_fps(now);
        assert!(
            (29.0..=31.0).contains(&achieved),
            "expected about 30 fps against the target, got {achieved}"
        );
        assert_eq!(live.target_fps(), 30);
    }
}
