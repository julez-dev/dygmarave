//! One sit-down with the keyboard: capture what it looked like, take it out of
//! any repainting LED mode, write frames, and put everything back.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::link::{Link, LinkError};
use crate::theme::{self, Rgb, ThemeError};

/// The only LED mode that leaves a written frame alone.
///
/// Modes 1–3 repaint the board on the firmware's own timer — measured on
/// v1.0.12-beta, a frame written in rainbow mode is replaced within a second.
pub const STATIC_MODE: u8 = 0;

/// How long to let a fire-and-forget frame land before reading it back. The
/// write is only guaranteed to have left this machine, not to have been applied.
const SETTLE: Duration = Duration::from_millis(120);

#[derive(Debug)]
pub enum SessionError {
    Link(LinkError),
    Theme(ThemeError),
    UnreadableMode(String),
    FrameSize { wrote: usize, board: usize },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Link(error) => write!(f, "{error}"),
            SessionError::Theme(error) => write!(f, "{error}"),
            SessionError::UnreadableMode(reply) => {
                write!(f, "the board reported an unreadable LED mode: {reply:?}")
            }
            SessionError::FrameSize { wrote, board } => write!(
                f,
                "refusing to write a {wrote}-LED frame to a board with {board} LEDs"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<LinkError> for SessionError {
    fn from(error: LinkError) -> Self {
        SessionError::Link(error)
    }
}

impl From<ThemeError> for SessionError {
    fn from(error: ThemeError) -> Self {
        SessionError::Theme(error)
    }
}

/// What the board looked like before we touched it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    pub mode: u8,
    pub theme: Vec<Rgb>,
}

/// Evidence that a written frame survived, or that the firmware painted over it.
#[derive(Debug, Clone)]
pub struct HoldReport {
    /// The colour that was written to every LED.
    pub written: Rgb,
    /// What the board reported shortly after the write.
    pub immediate: Vec<Rgb>,
    /// What the board reported after the hold.
    pub after: Vec<Rgb>,
}

impl HoldReport {
    /// LED indices that took the written colour.
    pub fn accepted(&self) -> usize {
        self.immediate
            .iter()
            .filter(|color| **color == self.written)
            .count()
    }

    /// LED indices that ignored the write entirely.
    pub fn unresponsive(&self) -> Vec<usize> {
        self.immediate
            .iter()
            .enumerate()
            .filter(|(_, color)| **color != self.written)
            .map(|(index, _)| index)
            .collect()
    }

    /// LED indices whose colour changed during the hold — the signature of an
    /// active effect painting over our frame.
    pub fn drifted(&self) -> Vec<usize> {
        self.immediate
            .iter()
            .zip(self.after.iter())
            .enumerate()
            .filter(|(_, (before, after))| before != after)
            .map(|(index, _)| index)
            .collect()
    }

    /// True when nothing changed during the hold, so the mode is static.
    pub fn is_static(&self) -> bool {
        self.drifted().is_empty()
    }
}

/// An open session that restores the board when [`Session::restore`] is called.
pub struct Session<L: Link> {
    link: L,
    baseline: Baseline,
    restored: bool,
}

impl<L: Link> Session<L> {
    /// Captures the baseline, then selects the static LED mode.
    ///
    /// The mode is switched before any frame is written, because in modes 1–3
    /// the firmware would paint over the first frame within a second.
    pub fn open(mut link: L) -> Result<Self, SessionError> {
        let mode_reply = link.query("led.mode")?;
        let mode = mode_reply
            .trim()
            .parse::<u8>()
            .map_err(|_| SessionError::UnreadableMode(mode_reply.clone()))?;
        let theme = theme::parse_theme_reply(&link.query("led.theme")?)?;

        link.send(&format!("led.mode {STATIC_MODE}"))?;

        Ok(Self {
            link,
            baseline: Baseline { mode, theme },
            restored: false,
        })
    }

    /// The LED count the board itself reported.
    pub fn led_count(&self) -> usize {
        self.baseline.theme.len()
    }

    pub fn baseline(&self) -> &Baseline {
        &self.baseline
    }

    /// The firmware version string, for the log.
    pub fn firmware(&mut self) -> Result<String, SessionError> {
        Ok(self.link.query("version")?.trim().to_string())
    }

    /// Writes one full frame, refusing a frame that does not match the board.
    pub fn write_frame(&mut self, colors: &[Rgb]) -> Result<(), SessionError> {
        if colors.len() != self.led_count() {
            return Err(SessionError::FrameSize {
                wrote: colors.len(),
                board: self.led_count(),
            });
        }
        self.link.send(&theme::encode_theme(colors))?;
        Ok(())
    }

    /// Writes a solid colour, holds it, and reports whether it survived.
    pub fn hold(
        &mut self,
        color: Rgb,
        duration: Duration,
        interrupted: &AtomicBool,
    ) -> Result<HoldReport, SessionError> {
        let frame = theme::uniform(self.led_count(), color);
        self.write_frame(&frame)?;
        sleep_unless_interrupted(SETTLE, interrupted);
        let immediate = theme::parse_theme_reply(&self.link.query("led.theme")?)?;

        sleep_unless_interrupted(duration, interrupted);
        let after = theme::parse_theme_reply(&self.link.query("led.theme")?)?;

        Ok(HoldReport {
            written: color,
            immediate,
            after,
        })
    }

    /// Puts the baseline theme and LED mode back. Safe to call twice.
    pub fn restore(&mut self) -> Result<(), SessionError> {
        if self.restored {
            return Ok(());
        }
        // Theme first, then mode: restoring the mode last lets the original
        // effect resume over the original colours rather than our last frame.
        self.link.send(&theme::encode_theme(&self.baseline.theme))?;
        self.link.send(&format!("led.mode {}", self.baseline.mode))?;
        self.restored = true;
        Ok(())
    }
}

impl<L: Link> Link for Session<L> {
    fn send(&mut self, command: &str) -> Result<(), LinkError> {
        self.link.send(command)
    }

    fn query(&mut self, command: &str) -> Result<String, LinkError> {
        self.link.query(command)
    }

    fn drain(&mut self) -> Result<usize, LinkError> {
        self.link.drain()
    }
}

/// Sleeps in small slices so an interrupt does not have to wait out a hold.
fn sleep_unless_interrupted(total: Duration, interrupted: &AtomicBool) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if interrupted.load(Ordering::SeqCst) {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeLink;

    fn baseline_theme(count: usize) -> String {
        theme::encode_theme(&theme::uniform(count, Rgb::new(10, 20, 30)))
            .trim_start_matches("led.theme ")
            .to_string()
    }

    fn opened(count: usize) -> Session<FakeLink> {
        let link = FakeLink::with_replies(&[
            ("led.mode", "1 \r\n"),
            ("led.theme", &baseline_theme(count)),
            ("version", "v1.0.12-beta\r\n"),
        ]);
        Session::open(link).expect("session opens")
    }

    #[test]
    fn learns_led_count_and_baseline_mode_from_the_board() {
        let mut session = opened(132);
        assert_eq!(session.led_count(), 132);
        assert_eq!(session.baseline().mode, 1);
        assert_eq!(session.baseline().theme[0], Rgb::new(10, 20, 30));
        assert_eq!(session.firmware().unwrap(), "v1.0.12-beta");
    }

    #[test]
    fn switches_to_static_mode_before_writing_any_frame() {
        let session = opened(132);
        let static_index = session
            .link
            .log
            .iter()
            .position(|command| command == "led.mode 0")
            .expect("static mode was selected");
        let first_frame = session
            .link
            .log
            .iter()
            .position(|command| command.starts_with("led.theme "));
        assert!(
            first_frame.is_none_or(|frame| static_index < frame),
            "mode switch must precede the first frame"
        );
    }

    #[test]
    fn refuses_a_frame_sized_for_a_different_board() {
        let mut session = opened(132);
        let error = session
            .write_frame(&theme::uniform(131, Rgb::new(1, 1, 1)))
            .unwrap_err();
        assert!(matches!(
            error,
            SessionError::FrameSize {
                wrote: 131,
                board: 132
            }
        ));
    }

    #[test]
    fn restore_rewrites_baseline_theme_then_mode() {
        let mut session = opened(4);
        session.restore().unwrap();
        let tail: Vec<&String> = session.link.log.iter().rev().take(2).rev().collect();
        assert!(tail[0].starts_with("led.theme "));
        assert_eq!(tail[1], "led.mode 1");
    }

    #[test]
    fn drain_reaches_the_underlying_port_through_the_session() {
        // A default trait method here would silently discard nothing, letting
        // acknowledgements pile up until the board stalls.
        let mut session = opened(4);
        session.link.pending_acks = 3;
        assert_eq!(session.drain().unwrap(), 3);
    }

    #[test]
    fn restore_is_idempotent() {
        let mut session = opened(4);
        session.restore().unwrap();
        let after_first = session.link.log.len();
        session.restore().unwrap();
        assert_eq!(session.link.log.len(), after_first);
    }

    #[test]
    fn hold_reports_a_static_board_as_static() {
        let link = FakeLink::with_replies(&[
            ("led.mode", "1 \r\n"),
            ("led.theme", &baseline_theme(4)),
        ]);
        let mut session = Session::open(link).unwrap();
        // The fake always answers the baseline, so nothing drifts.
        let report = session
            .hold(Rgb::new(1, 2, 3), Duration::ZERO, &AtomicBool::new(false))
            .unwrap();
        assert!(report.is_static());
    }

    #[test]
    fn drift_is_detected_when_the_board_paints_over_the_frame() {
        let report = HoldReport {
            written: Rgb::new(255, 0, 0),
            immediate: vec![Rgb::new(255, 0, 0), Rgb::new(0, 0, 0)],
            after: vec![Rgb::new(0, 213, 255), Rgb::new(0, 0, 0)],
        };
        assert_eq!(report.drifted(), vec![0]);
        assert!(!report.is_static());
        assert_eq!(report.accepted(), 1);
        assert_eq!(report.unresponsive(), vec![1]);
    }
}
