//! Frame-budget measurement.
//!
//! The number that matters is how fast full frames drain to the board when
//! nothing waits for an acknowledgement. Waiting for the `.` terminator is
//! measured separately in the ticket record; this module measures the path the
//! daemon actually uses.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::link::{Link, LinkError};
use crate::theme::Frame;

/// What a measurement run observed.
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub frames: u32,
    pub elapsed: Duration,
    pub frame_bytes: usize,
    /// Acknowledgements discarded during the run. The firmware replies to every
    /// command, frames included, so this grows with the frame rate.
    pub acks_drained: usize,
}

impl BenchResult {
    pub fn fps(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if self.frames == 0 || seconds <= 0.0 {
            return 0.0;
        }
        self.frames as f64 / seconds
    }

    pub fn ms_per_frame(&self) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        self.elapsed.as_secs_f64() * 1000.0 / self.frames as f64
    }

    pub fn kib_per_second(&self) -> f64 {
        let kib = (self.frames as f64 * self.frame_bytes as f64) / 1024.0;
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        kib / seconds
    }
}

/// How many frames may be written before acknowledgements are discarded. The
/// device answers every command, so a long run must clear them or the kernel
/// buffer fills and the firmware stalls.
pub const DRAIN_EVERY: u32 = 64;

/// Writes `frames` copies of `frame` as fast as the port accepts them.
///
/// Each `send` waits only for the bytes to leave this machine, never for the
/// device to respond, so the result is the throughput the visualizer can rely on.
pub fn measure<L: Link>(
    link: &mut L,
    frame: &Frame,
    frames: u32,
    interrupted: &AtomicBool,
) -> Result<BenchResult, LinkError> {
    let started = Instant::now();
    let mut sent = 0;
    let mut acks_drained = 0;
    for _ in 0..frames {
        if interrupted.load(Ordering::SeqCst) {
            break;
        }
        link.send(frame.body())?;
        sent += 1;
        if sent % DRAIN_EVERY == 0 {
            acks_drained += link.drain()?;
        }
    }
    Ok(BenchResult {
        frames: sent,
        elapsed: started.elapsed(),
        frame_bytes: frame.byte_len(),
        acks_drained,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeLink;

    fn tiny_frame() -> Frame {
        Frame::from_colors(&[crate::theme::Rgb::new(1, 2, 3)])
    }

    #[test]
    fn counts_frames_actually_sent() {
        let mut link = FakeLink::default();
        let result = measure(&mut link, &tiny_frame(), 5, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.frames, 5);
        assert_eq!(link.count("led.theme"), 5);
        assert_eq!(result.frame_bytes, tiny_frame().byte_len());
        assert_eq!(result.acks_drained, 0);
    }

    #[test]
    fn stops_when_interrupted() {
        let mut link = FakeLink::default();
        let flag = AtomicBool::new(true);
        let result = measure(&mut link, &tiny_frame(), 100, &flag).unwrap();
        assert_eq!(result.frames, 0);
        assert_eq!(result.fps(), 0.0);
    }

    #[test]
    fn surfaces_a_dead_port_instead_of_reporting_a_rate() {
        let mut link = FakeLink {
            fail_after: Some(3),
            ..FakeLink::default()
        };
        let error = measure(&mut link, &tiny_frame(), 10, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error, LinkError::Io("simulated dead port".to_string()));
    }

    #[test]
    fn discards_acknowledgements_periodically_so_the_port_cannot_fill() {
        let mut link = FakeLink {
            pending_acks: DRAIN_EVERY as usize,
            ..FakeLink::default()
        };
        let result = measure(
            &mut link,
            &tiny_frame(),
            DRAIN_EVERY,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.acks_drained, DRAIN_EVERY as usize);

        link.pending_acks = 0;
        let short = measure(&mut link, &tiny_frame(), 5, &AtomicBool::new(false)).unwrap();
        assert_eq!(short.acks_drained, 0);
    }

    #[test]
    fn derives_rates_from_a_known_duration() {
        let result = BenchResult {
            frames: 60,
            elapsed: Duration::from_secs(1),
            frame_bytes: 1330,
            acks_drained: 0,
        };
        assert!((result.fps() - 60.0).abs() < 1e-9);
        assert!((result.ms_per_frame() - 16.666).abs() < 0.01);
        assert!((result.kib_per_second() - 77.93).abs() < 0.1);
    }

    #[test]
    fn zero_frames_does_not_divide_by_zero() {
        let result = BenchResult {
            frames: 0,
            elapsed: Duration::ZERO,
            frame_bytes: 1330,
            acks_drained: 0,
        };
        assert_eq!(result.fps(), 0.0);
        assert_eq!(result.ms_per_frame(), 0.0);
        assert_eq!(result.kib_per_second(), 0.0);
    }
}
