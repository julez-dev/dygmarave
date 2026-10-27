//! One sit-down with the keyboard: capture what it looked like, take it out of
//! any repainting LED mode, write frames, and put everything back.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::bench::{self, BenchResult};
use crate::link::{Link, LinkError};
use crate::theme::{self, Frame, Rgb, ThemeError};

/// How long to let a fire-and-forget frame land before reading it back. The
/// write is only guaranteed to have left this machine, not to have been applied.
const SETTLE: Duration = Duration::from_millis(120);

/// The LED modes firmware v1.0.12-beta accepts.
///
/// Measured: only [`LedMode::Static`] leaves a written frame alone. The other
/// three repaint on the firmware's own timer and erase a frame within a second.
/// Anything above 3 is rejected by the firmware, which is why `dygma_focus`'s
/// enum (which names 4–6 as red/green/blue) is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedMode {
    Static,
    Rainbow,
    Cycle,
    Stalker,
    /// A mode this firmware version does not document.
    Other(u8),
}

impl LedMode {
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => LedMode::Static,
            1 => LedMode::Rainbow,
            2 => LedMode::Cycle,
            3 => LedMode::Stalker,
            other => LedMode::Other(other),
        }
    }

    pub const fn code(self) -> u8 {
        match self {
            LedMode::Static => 0,
            LedMode::Rainbow => 1,
            LedMode::Cycle => 2,
            LedMode::Stalker => 3,
            LedMode::Other(code) => code,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            LedMode::Static => "static",
            LedMode::Rainbow => "rainbow",
            LedMode::Cycle => "cycle",
            LedMode::Stalker => "stalker",
            LedMode::Other(_) => "undocumented",
        }
    }

    /// Whether a written frame survives in this mode.
    pub const fn holds_frames(self) -> bool {
        matches!(self, LedMode::Static)
    }
}

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
    pub mode: LedMode,
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

/// Reads the board's current theme and mode.
fn read_baseline<L: Link>(link: &mut L) -> Result<Baseline, SessionError> {
    let mode = mode_from_reply(link.query("led.mode")?)?;
    let theme = theme::parse_theme_reply(&link.query("led.theme")?)?;
    Ok(Baseline { mode, theme })
}

/// Parses the board's answer to `led.mode`.
fn mode_from_reply(reply: String) -> Result<LedMode, SessionError> {
    let code = reply
        .trim()
        .parse::<u8>()
        .map_err(|_| SessionError::UnreadableMode(reply.clone()))?;
    Ok(LedMode::from_code(code))
}

/// An open session that restores the board when it ends.
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
    pub fn open(link: L) -> Result<Self, SessionError> {
        Self::open_with(link, None)
    }

    /// Opens with a known baseline, for reconnects.
    ///
    /// A reconnect must restore the theme and mode that were in use *before the
    /// daemon started*, not whatever the board shows now. After a Bazecor pause or
    /// a replug the board still shows our last frame in static mode, so re-reading
    /// the baseline would capture our own output and restore it on exit. Passing
    /// the first baseline forward keeps the original safe.
    ///
    /// Still switches to static mode: Bazecor or a reset may have changed the mode
    /// while the daemon was away.
    pub fn open_with(link: L, baseline: Option<Baseline>) -> Result<Self, SessionError> {
        let mut link = link;
        let baseline = match baseline {
            Some(baseline) => baseline,
            None => read_baseline(&mut link)?,
        };

        link.send(&format!("led.mode {}", LedMode::Static.code()))?;

        Ok(Self {
            link,
            baseline,
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

    /// Proves the board is still answering, not merely accepting bytes.
    ///
    /// A wedged device can keep taking writes into the kernel buffer while
    /// nothing reaches the firmware, so a successful [`Session::write_frame`] is
    /// not proof the board is alive. A query must come back for the link to be
    /// trusted; anything else means the port is gone or frozen. The query drains
    /// first, so it also clears acknowledgements as a side effect.
    ///
    /// `led.mode` is read-only: the mode *setter* wears EEPROM, but reading it does
    /// not. On a wedged port the query blocks for the link's five-second reply
    /// timeout — the price of a definitive answer. The board is already frozen, so
    /// the delay costs nothing the picture was still showing.
    pub fn health_check(&mut self) -> Result<LedMode, SessionError> {
        mode_from_reply(self.link.query("led.mode")?)
    }

    /// A full frame of one colour, sized to this board.
    pub fn uniform_frame(&self, color: Rgb) -> Frame {
        Frame::uniform(self.led_count(), color)
    }

    /// Discards acknowledgements the board has already written.
    ///
    /// The board answers every frame, so a long run must clear them or the
    /// kernel buffer fills and the firmware stalls. The frame loop cannot afford
    /// a `query` (which drains first and then waits for a reply), so it calls
    /// this on a cadence instead — the same trick the benchmark uses.
    pub fn drain_acks(&mut self) -> Result<usize, SessionError> {
        Ok(self.link.drain()?)
    }

    /// Writes one full frame, refusing a frame that does not match the board.
    pub fn write_frame(&mut self, frame: &Frame) -> Result<(), SessionError> {
        if frame.leds() != self.led_count() {
            return Err(SessionError::FrameSize {
                wrote: frame.leds(),
                board: self.led_count(),
            });
        }
        self.link.send(frame.body())?;
        Ok(())
    }

    /// Writes a solid colour, holds it, and reports whether it survived.
    pub fn hold(
        &mut self,
        color: Rgb,
        duration: Duration,
        interrupted: &AtomicBool,
    ) -> Result<HoldReport, SessionError> {
        let frame = self.uniform_frame(color);
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

    /// Measures how fast this board accepts frames.
    pub fn measure(
        &mut self,
        frame: &Frame,
        frames: u32,
        interrupted: &AtomicBool,
    ) -> Result<BenchResult, LinkError> {
        bench::measure(&mut self.link, frame, frames, interrupted)
    }

    /// Closes the port without restoring the board.
    ///
    /// Used when another program takes the port over — Bazecor, say — and the
    /// daemon must let go without writing the baseline into a device it no longer
    /// owns. The board is left exactly as found: no restore commands are sent,
    /// and `Drop` stays quiet because the session is marked restored. Reopening a
    /// fresh [`Session`] later reads whatever the board now shows, so a daemon that
    /// was paused mid-show resumes against the colours it actually has.
    ///
    /// Dropping the link is what closes the port. The daemon keeps no reference to
    /// it afterwards, so reopening after a pause needs a freshly opened session.
    pub fn release(mut self) {
        // Nothing to send. Marking the session restored stops `Drop` restoring a
        // baseline this process no longer owns.
        self.restored = true;
    }

    /// Puts the baseline theme and LED mode back. Safe to call twice.
    pub fn restore(&mut self) -> Result<(), SessionError> {
        if self.restored {
            return Ok(());
        }
        // Theme first, then mode: restoring the mode last lets the original
        // effect resume over the original colours rather than our last frame.
        self.link.send(&theme::encode_theme(&self.baseline.theme))?;
        self.link
            .send(&format!("led.mode {}", self.baseline.mode.code()))?;
        self.restored = true;
        Ok(())
    }
}

impl<L: Link> Drop for Session<L> {
    /// Best-effort restore on any exit path that did not call [`Session::restore`],
    /// including an error or a panic later in the run.
    ///
    /// A `Drop` cannot report failure, so a port that has already gone away
    /// leaves the board in the static mode we selected; ticket 08 owns retrying
    /// there. Leaving it out entirely would mean a mid-run error always left the
    /// keyboard in our colours, which is the more common failure.
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.restore();
        }
    }
}

/// Sleeps in small slices so an interrupt does not have to wait out a frame.
pub(crate) fn sleep_unless_interrupted(total: Duration, interrupted: &AtomicBool) {
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
    use crate::testing::{FakeLink, SharedLink};

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
    fn mode_codes_round_trip() {
        for code in 0..=7u8 {
            assert_eq!(LedMode::from_code(code).code(), code);
        }
        assert_eq!(LedMode::from_code(0), LedMode::Static);
        assert_eq!(LedMode::from_code(4), LedMode::Other(4));
        assert!(LedMode::Static.holds_frames());
        assert!(!LedMode::Rainbow.holds_frames());
    }

    #[test]
    fn learns_led_count_and_baseline_mode_from_the_board() {
        let mut session = opened(132);
        assert_eq!(session.led_count(), 132);
        assert_eq!(session.baseline().mode, LedMode::Rainbow);
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
            .write_frame(&Frame::uniform(131, Rgb::new(1, 1, 1)))
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
    fn writing_a_frame_is_fire_and_forget() {
        // The board acknowledges every frame, and waiting for that ack would halve
        // the achievable rate: ticket 01 measured ~38 fps request/response against
        // ~70 fire-and-forget. A frame write must be one send and nothing else.
        let mut session = opened(4);
        let sent = session.link.log.len();
        session
            .write_frame(&Frame::uniform(4, Rgb::new(1, 2, 3)))
            .unwrap();
        assert_eq!(
            session.link.log.len(),
            sent + 1,
            "one command, no read-back"
        );
        assert!(session.link.log.last().unwrap().starts_with("led.theme "));
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
    fn an_unfinished_session_puts_the_board_back_when_dropped() {
        let link =
            SharedLink::with_replies(&[("led.mode", "1 \r\n"), ("led.theme", &baseline_theme(4))]);
        let mut session = Session::open(link.clone()).unwrap();
        session
            .write_frame(&Frame::uniform(4, Rgb::new(9, 9, 9)))
            .unwrap();
        drop(session);
        let log = link.log();
        let theme_calls: Vec<&String> = log
            .iter()
            .filter(|command| command.starts_with("led.theme "))
            .collect();
        assert_eq!(
            theme_calls.len(),
            2,
            "the frame written plus the restored baseline"
        );
        assert_eq!(log.last().unwrap(), "led.mode 1");
    }

    #[test]
    fn drain_acks_clears_what_the_board_has_written() {
        // The frame loop calls this on a cadence instead of querying, so it must
        // actually reach the port and clear the pending acknowledgements.
        let mut session = opened(4);
        session.link.pending_acks = 3;
        assert_eq!(session.drain_acks().unwrap(), 3);
        assert_eq!(session.drain_acks().unwrap(), 0, "nothing left to discard");
    }

    #[test]
    fn drain_reaches_the_underlying_port_through_the_session() {
        // Measuring borrows the session's link directly, so this asserts the
        // wiring that keeps acknowledgements from piling up.
        let mut session = opened(4);
        session.link.pending_acks = 3;
        let drained = bench::measure(
            &mut session.link,
            &Frame::uniform(4, Rgb::new(1, 1, 1)),
            bench::DRAIN_EVERY,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(drained.acks_drained, 3);
    }

    #[test]
    fn a_health_check_proves_the_board_still_answers() {
        let mut session = opened(4);
        // The board answers a *fresh* query, not just accepts another write.
        session
            .link
            .replies
            .insert("led.mode".into(), "0 \r\n".into());
        let queries_before = session.link.count("led.mode");
        assert_eq!(session.health_check().unwrap(), LedMode::Static);
        // It is a real query, so it appears in the log after the open's own.
        assert_eq!(session.link.count("led.mode"), queries_before + 1);
        assert_eq!(session.link.log.last().unwrap(), "led.mode");
    }

    #[test]
    fn a_health_check_reports_a_port_that_stops_answering() {
        let mut session = opened(4);
        // The port accepts what it has already been given and then goes away.
        session.link.fail_after = Some(session.link.log.len());
        let error = session.health_check().unwrap_err();
        assert!(
            matches!(error, SessionError::Link(_)),
            "a dead port must surface as a link error, got {error:?}"
        );
    }

    #[test]
    fn a_health_check_rejects_a_reply_it_cannot_parse() {
        let mut session = opened(4);
        session
            .link
            .replies
            .insert("led.mode".into(), "???\r\n".into());
        assert!(matches!(
            session.health_check().unwrap_err(),
            SessionError::UnreadableMode(_)
        ));
    }

    #[test]
    fn reopening_with_a_known_baseline_skips_the_read() {
        // A reconnect keeps the pre-startup baseline; the board now shows our last
        // frame in static mode, so re-reading would capture our own output.
        let link = SharedLink::with_replies(&[]);
        let baseline = Baseline {
            mode: LedMode::Rainbow,
            theme: vec![Rgb::new(1, 2, 3)],
        };
        let session = Session::open_with(link.clone(), Some(baseline.clone())).unwrap();
        assert_eq!(session.baseline(), &baseline);
        assert_eq!(
            link.log(),
            vec!["led.mode 0".to_string()],
            "no read, just the switch to static"
        );
    }

    #[test]
    fn releasing_the_port_restores_nothing() {
        // The daemon lets go when Bazecor takes the port. Releasing must not write
        // the baseline into a device another program now owns, and the `Drop` that
        // follows must not either.
        let link =
            SharedLink::with_replies(&[("led.mode", "1 \r\n"), ("led.theme", &baseline_theme(4))]);
        let mut session = Session::open(link.clone()).unwrap();
        session
            .write_frame(&Frame::uniform(4, Rgb::new(9, 9, 9)))
            .unwrap();
        let commands = link.log().len();
        session.release();
        assert_eq!(
            link.log().len(),
            commands,
            "release must send nothing, and Drop must stay quiet"
        );
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
        let link =
            FakeLink::with_replies(&[("led.mode", "1 \r\n"), ("led.theme", &baseline_theme(4))]);
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
