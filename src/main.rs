//! dygmarave — audio-reactive LEDs on a Dygma Raise.
//!
//! Three readouts exist so far, each covering one seam of the eventual pipeline:
//!
//! * the default `LED path` finds the board by USB identity, learns its LED count
//!   from the board itself, holds a written frame long enough to prove no active
//!   effect paints over it, measures full-frame throughput, and restores the
//!   keyboard exactly as it was found (ticket 01);
//! * `--probe-audio` captures the output mix and prints a live level meter,
//!   refusing microphones (ticket 02);
//! * `--probe-spectrum` prints 28 log-spaced band energies per channel, the
//!   analysis the renderer will consume (ticket 03);
//! * `--live` is the product: the output mix drives the board continuously,
//!   band energies through the colour ramp at an adaptive frame rate that lowers
//!   itself when the board cannot keep up (tickets 05 and 07).

mod args;
mod audio;
mod bazecor;
mod bench;
mod config;
mod dynamics;
mod layout;
mod level;
mod link;
mod live;
mod look;
mod motion;
mod palette;
mod render;
mod session;
mod spectrum;
mod theme;

#[cfg(test)]
mod testing;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};

use audio::{verdict, AudioError, SourceKind, Verdict};
use link::{DygmaLink, Link};
use session::{Baseline, Session};
use theme::{Frame, Rgb};

fn main() -> ExitCode {
    // clap owns the argument surface: it renders --help and --version to stdout and
    // usage errors to stderr, each with its conventional exit code. A parse error
    // never reaches run().
    let args = match args::parse_args(std::env::args_os()) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // `{:#}` prints the whole context chain, not just the outermost message.
            eprintln!("dygmarave: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Opens the board from the configured port, or by USB discovery.
fn open_device(config: &config::Config) -> anyhow::Result<DygmaLink> {
    match config.port() {
        Some(port) => DygmaLink::from_port(port).context("opening the configured port"),
        None => DygmaLink::discover().context("finding the keyboard"),
    }
}

fn run(args: args::Args) -> anyhow::Result<()> {
    // Load the config, then let explicit CLI flags override it. A malformed or
    // out-of-range file is an error here, before the board is touched.
    let mut config = config::Config::load_default().context("loading the configuration")?;
    if let Some(port) = &args.port {
        config.device.port = port.clone();
    }
    if let Some(source) = &args.source {
        config.audio.source = source.clone();
    }
    if let Some(fps) = args.fps {
        config.render.fps_target = fps;
    }
    // CLI look flags override the file's axis choice. Pools and intervals stay
    // config-only; "random" here means "draw from that configured pool".
    if let Some(layout) = &args.layout {
        config.look.layout = layout.clone();
    }
    if let Some(palette) = &args.palette {
        config.look.palette = palette.clone();
    }
    if let Some(motion) = &args.motion {
        config.look.motion = motion.clone();
    }

    let interrupted = install_interrupt_handler(if args.probe_audio {
        "stopping"
    } else {
        "restoring the keyboard"
    })
    .context("installing the Ctrl-C handler")?;

    if args.probe_columns || args.probe_rows || args.probe_gradient || args.probe_thumbs {
        return paint_pattern(&args, &config, &interrupted);
    }
    if args.probe_audio {
        return probe_audio(&args, &config, &interrupted);
    }
    if args.live {
        return live_visualizer(&config, &args, &interrupted);
    }

    let device = open_device(&config)?;
    println!("port: {}", device.port_name());

    let mut session = Session::open(device).context("opening a session with the board")?;
    println!("firmware: {}", session.firmware()?);
    let count = session.led_count();
    println!("LED count: {count} (reported by the board, not assumed)");
    println!(
        "baseline: LED mode {} ({}, {}), {} distinct colours",
        session.baseline().mode.code(),
        session.baseline().mode.name(),
        if session.baseline().mode.holds_frames() {
            "holds frames"
        } else {
            "repaints frames"
        },
        Rgb::distinct(&session.baseline().theme)
    );
    println!(
        "switched to static LED mode ({})",
        session::LedMode::Static.code()
    );

    // Prove a frame survives. Without this the frame budget below would be a
    // measurement of our own syscalls while an effect painted over all of them.
    let hold = Duration::from_millis(args.hold_ms);
    println!(
        "\nholding {} for {} ms to check it survives…",
        args.colour, args.hold_ms
    );
    let report = session.hold(args.colour, hold, &interrupted)?;
    println!(
        "  accepted by {} of {} LEDs",
        report.accepted(),
        report.immediate.len()
    );
    if !report.unresponsive().is_empty() {
        println!(
            "  never take a theme colour: {:?} (the board ignores writes to these)",
            report.unresponsive()
        );
    }
    let drifted = report.drifted();
    if report.is_static() {
        println!("  static: no LED changed during the hold");
    } else {
        println!(
            "  WARNING: {} LEDs changed during the hold — an effect is repainting over the frame",
            drifted.len()
        );
    }

    // The frame budget.
    let frame = Frame::uniform(count, args.colour);
    println!(
        "\nmeasuring {} full frames of {} bytes, fire-and-forget…",
        args.frames,
        frame.byte_len()
    );
    let result = session.measure(&frame, args.frames, &interrupted)?;
    if interrupted.load(Ordering::SeqCst) {
        println!("interrupted after {} frames", result.frames);
    }
    println!(
        "  {} frames in {} ms",
        result.frames,
        result.elapsed.as_millis()
    );
    println!(
        "  {:.1} fps, {:.2} ms per frame",
        result.fps(),
        result.ms_per_frame()
    );
    println!("  {:.0} KiB/s of LED data", result.kib_per_second());
    println!(
        "  {} acknowledgements discarded (the board acks every frame)",
        result.acks_drained
    );

    // Put the board back the way it was found.
    session.restore()?;
    println!(
        "\nrestored baseline theme and LED mode {} ({})",
        session.baseline().mode.code(),
        session.baseline().mode.name()
    );
    Ok(())
}

/// Paints a diagnostic pattern on the board and holds it, so its geometry can be
/// checked by eye or in a photograph.
fn paint_pattern(
    args: &args::Args,
    config: &config::Config,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    let device = open_device(config)?;
    println!("port: {}", device.port_name());
    let mut session = Session::open(device).context("opening a session with the board")?;
    let count = session.led_count();
    println!(
        "LED count: {count}; the grid claims {} key LEDs across {} rows x {} columns",
        layout::key_leds().len(),
        layout::ROWS,
        layout::COLUMNS
    );

    if args.probe_gradient {
        for half in [layout::Half::Left, layout::Half::Right] {
            let leds = layout::band_leds(half);
            let lowest = leds.first().copied().unwrap_or_default();
            let highest = leds.last().copied().unwrap_or_default();
            println!(
                "  {} half: {} bands, band 0 on LED {lowest} (inner), last band on LED {highest} (outer)",
                half.name(),
                leds.len()
            );
        }
        println!();
    }

    let colors = if args.probe_gradient {
        layout::gradient(count)
    } else if args.probe_thumbs {
        println!(
            "\n  guessed thumb clusters — left magenta, right cyan; the dim grey is the rest of the underglow"
        );
        layout::thumb_pattern(count)
    } else if args.probe_rows {
        println!("\n  row   colour (r,g,b)");
        for row in 0..layout::ROWS {
            println!("  {row}     {}", layout::row_colour(row));
        }
        layout::row_pattern(count)
    } else {
        println!("\n  column   half     colour (r,g,b)");
        for column in 0..layout::COLUMNS {
            let half = if column < 8 {
                layout::Half::Left
            } else {
                layout::Half::Right
            };
            println!(
                "  {column:>6}   {:<6}   {}",
                half.name(),
                layout::column_colour(column)
            );
        }
        layout::column_pattern(count)
    };

    session.write_frame(&Frame::from_colors(&colors))?;
    println!(
        "\n{}",
        if args.probe_gradient {
            "Red = lowest band at the inner edge, blue = highest at the outer edges."
        } else if args.probe_thumbs {
            "Take a photograph: the magenta and cyan LEDs are where the thumbs are guessed to be.\n  If they are in the wrong place, the indices in layout.rs need correcting."
        } else {
            "Every key in one group shares a colour; underglow and thumbs stay dark."
        }
    );
    println!("Holding {} s (Ctrl-C to end early)…", args.seconds);
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    while Instant::now() < deadline && !interrupted.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(25));
    }
    session.restore()?;
    println!(
        "restored baseline theme and LED mode {}",
        session.baseline().mode.code()
    );
    Ok(())
}

/// Builds the band plan from the `[analysis]` section.
///
/// The window and band edges are user-tunable; `BandPlan::standard` hardcodes the
/// project defaults, so using it here would make those keys silent no-ops.
fn band_plan(config: &config::Config, sample_rate: u32) -> spectrum::BandPlan {
    spectrum::BandPlan::new(
        sample_rate,
        config.analysis.window,
        spectrum::BAND_COUNT,
        config.analysis.band_min_hz,
        config.analysis.band_max_hz,
    )
}

/// What a probe draws while it runs.
enum Probe {
    Level {
        window: Vec<f32>,
        held: Vec<f32>,
        loudest: f32,
    },
    Spectrum {
        analyzer: Box<spectrum::Analyzer>,
        /// Loudest band seen per channel, and its energy.
        loudest: Vec<(usize, f32)>,
        peak_energy: f32,
    },
}

impl Probe {
    fn new(config: &config::Config, args: &args::Args, capture: &audio::Capture) -> Self {
        if args.probe_spectrum {
            let plan = band_plan(config, capture.sample_rate);
            println!(
                "  window: {} samples ({:.1} ms), {} bands {:.0}-{:.0} Hz, log-spaced",
                plan.fft_size(),
                plan.fft_size() as f32 * 1000.0 / capture.sample_rate as f32,
                plan.band_count(),
                plan.edges()[0],
                plan.edges()[plan.band_count()]
            );
            Probe::Spectrum {
                analyzer: Box::new(spectrum::Analyzer::new(plan, capture.channels as usize)),
                loudest: vec![(0, 0.0); capture.channels as usize],
                peak_energy: 0.0,
            }
        } else {
            Probe::Level {
                window: Vec::new(),
                held: vec![0.0; capture.channels as usize],
                loudest: 0.0,
            }
        }
    }

    fn absorb(&mut self, samples: &[f32]) {
        match self {
            Probe::Level { window, .. } => window.extend_from_slice(samples),
            Probe::Spectrum { analyzer, .. } => analyzer.push_interleaved(samples),
        }
    }

    /// Draws the current state, updating what has been heard.
    fn line(&mut self, channels: u16) -> Option<String> {
        match self {
            Probe::Level {
                window,
                held,
                loudest,
            } => {
                let levels = level::measure(window, channels)?;
                *loudest = loudest.max(levels.loudest());
                for (index, peak) in levels.peak.iter().enumerate() {
                    if let Some(slot) = held.get_mut(index) {
                        *slot = slot.max(*peak);
                    }
                }
                window.clear();
                Some(level::meter_line(&levels))
            }
            Probe::Spectrum {
                analyzer,
                loudest,
                peak_energy,
            } => {
                let spectrum = analyzer.spectrum()?;
                for (channel, slot) in loudest.iter_mut().enumerate() {
                    let (band, level) = spectrum.loudest_band(channel);
                    *peak_energy = peak_energy.max(level);
                    if level >= slot.1 {
                        *slot = (band, level);
                    }
                }
                Some(render_bands(spectrum))
            }
        }
    }

    fn loudest(&self) -> f32 {
        match self {
            Probe::Level { loudest, .. } => *loudest,
            Probe::Spectrum { peak_energy, .. } => *peak_energy,
        }
    }

    fn summary(&self) -> Vec<String> {
        match self {
            Probe::Level { held, .. } => held
                .iter()
                .enumerate()
                .map(|(index, peak)| {
                    format!(
                        "  channel {} peak: {:.1} dBFS",
                        index + 1,
                        level::to_dbfs(*peak)
                    )
                })
                .collect(),
            Probe::Spectrum {
                analyzer, loudest, ..
            } => {
                let plan = analyzer.plan();
                loudest
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, level))| *level > 0.0)
                    .map(|(index, (band, level))| {
                        format!(
                            "  channel {} loudest band: {} of {} (~{:.0} Hz, {:.3})",
                            index + 1,
                            band,
                            plan.band_count(),
                            plan.centre_hz(*band),
                            level
                        )
                    })
                    .collect()
            }
        }
    }
}

/// One character per band, so a sweep is visible as a moving peak.
fn render_bands(spectrum: &spectrum::Spectrum) -> String {
    (0..spectrum.channel_count())
        .map(|channel| {
            spectrum
                .bands(channel)
                .iter()
                .map(|amplitude| level::bar(level::to_dbfs(*amplitude), 1))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_string()
}

/// Captures the output mix and prints a live readout, so the audio path and the
/// analysis can be checked before the renderer exists.
fn probe_audio(
    args: &args::Args,
    config: &config::Config,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    let selection = audio::choose(config.source()).context("choosing an audio source")?;
    println!(
        "source: {selection}{}",
        if selection.follows_default_sink() {
            " — the default sink's monitor, as chosen at startup"
        } else {
            ""
        }
    );
    if selection.kind() == SourceKind::Unknown {
        println!("  note: this device is not recognisable as a monitor; capturing it because you named it");
    }

    let (sender, receiver) = mpsc::channel();
    let capture = audio::start(&selection, sender).context("starting audio capture")?;
    println!(
        "  {} channels at {} Hz, {:?}",
        capture.channels, capture.sample_rate, capture.sample_format
    );
    let mut probe = Probe::new(config, args, &capture);
    println!("  listening for {} s — play something…", args.seconds);

    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut blocks = 0u64;
    let mut last_draw = Instant::now();
    // A live readout redraws in place, but a journal or a pipe gets one line a
    // second instead of a wall of carriage returns.
    let interactive = std::io::stdout().is_terminal();
    let redraw_every = if interactive {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(1)
    };

    while Instant::now() < deadline && !interrupted.load(Ordering::SeqCst) {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(block) => {
                blocks += 1;
                probe.absorb(&block.samples);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_draw.elapsed() < redraw_every {
            continue;
        }
        if let Some(line) = probe.line(capture.channels) {
            if interactive {
                print!("\r{line}    ");
            } else {
                println!("{line}");
            }
            std::io::stdout().flush()?;
        }
        last_draw = Instant::now();
    }
    // The final window counts too: audio that started just before the deadline
    // is still audio, and discarding it reported "heard nothing" for a source
    // that had plainly been heard.
    if let Some(line) = probe.line(capture.channels) {
        if !interactive {
            println!("{line}");
        }
    }
    if interactive {
        println!();
    }
    let stream_error = capture.take_error();

    if verdict(probe.loudest()) == Verdict::Silent {
        if let Some(error) = stream_error {
            println!("  stream stopped: {error}");
        }
        return Err(AudioError::NoSignal {
            selection: Box::new(selection),
            blocks,
        }
        .into());
    }

    if let Some(error) = stream_error {
        println!("  note: the stream reported an error: {error}");
    }
    for line in probe.summary() {
        println!("{line}");
    }
    println!(
        "  captured {} at {:.1} dBFS peak — this is the output mix, not the microphone",
        selection.display_name(),
        level::to_dbfs(probe.loudest())
    );
    Ok(())
}

/// How often to discard the board's acknowledgements. The board acks every frame,
/// so over an hour-long show the kernel buffer would fill and stall the port; a
/// quarter second keeps it far from full and costs one short read four times a
/// second. A frame count would stretch when the adaptive rate lowers.
const DRAIN_INTERVAL: Duration = Duration::from_millis(250);

/// How often to prove the board is still answering. A wedged port can keep
/// accepting writes into the kernel buffer while nothing reaches the firmware, so
/// a periodic query is the only proof the board is alive.
const PROBE_INTERVAL: Duration = Duration::from_secs(10);

/// How often to say what rate is actually being achieved, so a slow board is
/// visible rather than mysterious.
const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// A wall-clock cadence for work that must happen regularly but not every frame.
struct Cadence {
    last: Instant,
    every: Duration,
}

impl Cadence {
    fn new(every: Duration, start: Instant) -> Self {
        Self { last: start, every }
    }

    /// True when `every` has elapsed since the last time this returned true.
    fn due(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.last) < self.every {
            return false;
        }
        self.last = now;
        true
    }
}

/// How often to check whether Bazecor has taken the port while running.
const BAZECOR_INTERVAL: Duration = Duration::from_secs(1);

/// How often to look while waiting for the board or Bazecor to become available.
const RETRY_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Exponential backoff between reconnection attempts, capped so the daemon is not
/// asleep for minutes after the board returns.
struct Backoff {
    next: Duration,
}

impl Backoff {
    const FIRST: Duration = Duration::from_secs(1);
    const MAX: Duration = Duration::from_secs(30);

    /// The delay to wait before the next attempt, doubling up to the cap.
    fn delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(Self::MAX);
        delay
    }

    /// Resets after a successful connection, so the next loss retries promptly.
    fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: Self::FIRST }
    }
}

/// Reports a fatal audio-stream error, if one has occurred.
///
/// `run_frames` needs this rather than a concrete [`audio::Capture`] so a test can
/// drive it without a sound card. Production impls delegate to the capture's own
/// error slot.
trait AudioHealth {
    fn error(&self) -> Option<String>;
}

impl AudioHealth for audio::Capture {
    fn error(&self) -> Option<String> {
        self.take_error()
    }
}

/// Why one run of the frame loop ended.
#[derive(Debug)]
enum Detach {
    /// The user interrupted; the board should be restored.
    Interrupted,
    /// Bazecor took the port; it must be released without restoring.
    Paused,
    /// The port failed; the caller should reconnect.
    Lost(String),
}

/// Opens the board, returning the session and the port name for the log.
///
/// `baseline` is the theme and mode from the first successful open. Passing it on
/// a reconnect keeps the original safe: the board now shows our last frame in
/// static mode, so re-reading would capture our own output.
fn open_session(
    config: &config::Config,
    baseline: Option<Baseline>,
) -> anyhow::Result<(Session<DygmaLink>, String)> {
    let device = open_device(config)?;
    let port_name = device.port_name().to_string();
    let session =
        Session::open_with(device, baseline).context("opening a session with the board")?;
    Ok((session, port_name))
}

/// Drains and discards the audio backlog so a long pause does not grow memory.
fn discard_audio(receiver: &Receiver<audio::Block>) {
    while receiver.try_recv().is_ok() {}
}

/// Waits out the backoff delay, returning true when interrupted.
fn sleep_backoff(backoff: &mut Backoff, interrupted: &AtomicBool) -> bool {
    let delay = backoff.delay();
    session::sleep_unless_interrupted(delay, interrupted);
    interrupted.load(Ordering::SeqCst)
}

/// A port failure: report it so the caller can reconnect.
fn lost<E: std::fmt::Display>(error: E, acks: u64) -> anyhow::Result<(Detach, u64)> {
    Ok((Detach::Lost(error.to_string()), acks))
}

/// Fails if the audio stream has died. Losing audio is fatal, unlike losing the board.
fn check_audio(audio: &dyn AudioHealth, frames: u64) -> anyhow::Result<()> {
    match audio.error() {
        Some(error) => {
            bail!("audio stream stopped mid-set after {frames} frames: {error}")
        }
        None => Ok(()),
    }
}

/// Runs the frame loop until interrupted, Bazecor appears, or the port fails.
///
/// Returns the reason it stopped and how many acknowledgements were discarded.
/// An audio-stream failure is returned as an error: losing the audio is fatal,
/// unlike losing the board.
fn run_frames<L: Link>(
    session: &mut Session<L>,
    visualizer: &mut live::Live,
    receiver: &Receiver<audio::Block>,
    audio: &dyn AudioHealth,
    interrupted: &AtomicBool,
) -> anyhow::Result<(Detach, u64)> {
    let mut drain = Cadence::new(DRAIN_INTERVAL, Instant::now());
    let mut probe = Cadence::new(PROBE_INTERVAL, Instant::now());
    let mut report = Cadence::new(REPORT_INTERVAL, Instant::now());
    let mut bazecor_check = Cadence::new(BAZECOR_INTERVAL, Instant::now());
    let mut acks = 0u64;

    while !interrupted.load(Ordering::SeqCst) {
        let now = Instant::now();
        // Bazecor first: stop writing before it corrupts the command stream. The
        // session must be released without restoring, because the board is no
        // longer ours to paint.
        if bazecor_check.due(now) && bazecor::running() {
            return Ok((Detach::Paused, acks));
        }

        // Drain every block in order so the analyzer's window stays contiguous;
        // only a multi-second stall trips the cap and drops the oldest whole
        // blocks. Draining to just the newest block would splice two moments of
        // audio together and click, which reads as strobing at the frame rate.
        let (latest, dropped) = live::Live::drain(receiver);
        let frame = visualizer.step(latest.as_ref(), now, dropped);
        if let Err(error) = session.write_frame(&frame) {
            return lost(error, acks);
        }

        // Pacing after the write, so the board's write time is absorbed by the
        // schedule instead of pushing every frame past its deadline. This may
        // lower or raise the rate when the board cannot keep up.
        let before = visualizer.fps();
        let wait = visualizer.wait(now);
        let after = visualizer.fps();
        if after < before {
            println!(
                "  board falling behind — lowered frame rate {before} → {after} fps (spectrum unchanged)"
            );
        } else if after > before {
            println!("  board keeping up — raised frame rate {before} → {after} fps");
        }

        // Discard acknowledgements unconditionally, on a wall-clock cadence.
        if drain.due(now) {
            match session.drain_acks() {
                Ok(count) => acks += count as u64,
                Err(error) => return lost(error, acks),
            }
        }
        // Prove the board is still answering, not just accepting writes. The
        // query drains first, so it also clears acknowledgements as a side effect.
        if probe.due(now) {
            if let Err(error) = session.health_check() {
                return lost(error, acks);
            }
        }
        // Report the rate over the last observation window, not an average that an
        // hour of healthy running would hide a recent slowdown behind.
        if report.due(now) {
            match visualizer.recent_fps() {
                Some(recent) => println!(
                    "  {recent:.1} fps in the last window against a {} fps target",
                    visualizer.target_fps()
                ),
                None => println!(
                    "  {:.1} fps achieved so far against a {} fps target",
                    visualizer.achieved_fps(now),
                    visualizer.target_fps()
                ),
            }
        }

        // A mid-set stream failure is not "no signal was ever heard" — that error
        // means every sample was zero. Losing the audio is fatal, unlike losing the
        // board.
        check_audio(audio, visualizer.frames())?;
        session::sleep_unless_interrupted(wait, interrupted);
    }
    Ok((Detach::Interrupted, acks))
}

/// The one startup line that makes a random rotation reproducible.
fn seed_line(seed: u64) -> String {
    format!("seed={seed}")
}

/// Runs the visualizer until interrupted: audio in, frames out, steady pace.
///
/// The audio source is resolved *before* the board is touched, so a failed
/// capture leaves the keyboard alone rather than flashing it and restoring.
///
/// The board comes and goes: Bazecor may take the port, or the board may be
/// unplugged. The audio pipeline stays up throughout; the outer loop waits and
/// reopens the board when it is available again.
fn live_visualizer(
    config: &config::Config,
    args: &args::Args,
    interrupted: &AtomicBool,
) -> anyhow::Result<()> {
    // Without --seed, fastrand draws fresh OS entropy. Log the effective seed once
    // so a surprising random rotation can be reproduced.
    let seed = args.seed.unwrap_or_else(|| fastrand::u64(..));
    if !args.quiet {
        eprintln!("{}", seed_line(seed));
    }
    let look_settings = config.look_settings();

    let selection = audio::choose(config.source()).context("choosing an audio source")?;
    println!("source: {selection}");

    let (sender, receiver) = mpsc::channel();
    let capture = audio::start(&selection, sender).context("starting audio capture")?;
    println!(
        "  {} channels at {} Hz — drive the board until Ctrl-C",
        capture.channels, capture.sample_rate
    );

    let plan = band_plan(config, capture.sample_rate);
    let live_config = live::LiveConfig {
        fps: config.render.fps_target,
        channels: capture.channels as usize,
    };

    let started = Instant::now();
    let mut backoff = Backoff::default();
    // The theme and mode from the first successful open. A reconnect must restore
    // this, not whatever the board shows now — after a pause that is our own frame.
    let mut original: Option<Baseline> = None;
    let mut total_frames = 0u64;
    let mut total_acks = 0u64;
    let mut total_blocks = 0u64;
    let mut total_resyncs = 0u64;
    let mut last_rate: Option<(u32, u32, f64)> = None;

    while !interrupted.load(Ordering::SeqCst) {
        // Bazecor owns the port. Pause writes rather than corrupt its command
        // stream, keep the audio pipeline alive, and discard the backlog so a long
        // pause does not grow memory.
        if bazecor::running() {
            println!("  Bazecor is running — pausing writes until it exits");
            while bazecor::running() && !interrupted.load(Ordering::SeqCst) {
                discard_audio(&receiver);
                session::sleep_unless_interrupted(RETRY_POLL_INTERVAL, interrupted);
                // An audio stream that dies during a pause is still fatal.
                check_audio(&capture, total_frames)?;
            }
            if interrupted.load(Ordering::SeqCst) {
                break;
            }
            println!("  Bazecor exited — resuming");
        }

        // Try to open the board. A failure means it is unplugged or the port is
        // busy; keep the audio pipeline alive and retry with backoff.
        let (mut session, port_name) = match open_session(config, original.clone()) {
            Ok(opened) => {
                backoff.reset();
                // Remember the pre-startup baseline on the first open, and keep it
                // across reconnects.
                if original.is_none() {
                    original = Some(opened.0.baseline().clone());
                }
                opened
            }
            Err(error) => {
                println!("  board unavailable ({error}) — retrying");
                discard_audio(&receiver);
                if sleep_backoff(&mut backoff, interrupted) {
                    break;
                }
                check_audio(&capture, total_frames)?;
                continue;
            }
        };

        let count = session.led_count();
        let mut visualizer = live::Live::with_look(
            live_config,
            config.dynamics(),
            look_settings.clone(),
            seed,
            args.quiet,
            plan.clone(),
            count,
            Instant::now(),
        );
        println!(
            "  port {port_name} — {count} LEDs, {} fps, {} bands",
            visualizer.fps(),
            visualizer.bands()
        );

        // Run frames until something detaches: the user, Bazecor, or the port.
        let (detach, acks) = run_frames(
            &mut session,
            &mut visualizer,
            &receiver,
            &capture,
            interrupted,
        )?;
        total_frames += visualizer.frames();
        total_acks += acks;
        total_blocks += visualizer.dropped_blocks();
        total_resyncs += visualizer.resyncs();
        last_rate = Some((
            visualizer.fps(),
            visualizer.target_fps(),
            visualizer.achieved_fps(Instant::now()),
        ));

        match detach {
            Detach::Interrupted => {
                // A clean stop: restore the board. If the port is already gone the
                // restore cannot succeed, but the user asked to stop, so report it
                // and exit cleanly rather than tripping systemd's restart policy.
                match session.restore() {
                    Ok(()) => println!(
                        "  restored LED mode {} ({})",
                        session.baseline().mode.code(),
                        session.baseline().mode.name()
                    ),
                    Err(error) => eprintln!("  could not restore the board: {error}"),
                }
                break;
            }
            Detach::Paused => {
                // Bazecor owns the port now. Let go without writing the baseline
                // into a device another program controls; the top of the loop waits
                // for Bazecor to exit and reopens.
                session.release();
                // The top of the outer loop waits for Bazecor to exit.
                continue;
            }
            Detach::Lost(error) => {
                // The port failed. Dropping the session attempts a best-effort
                // restore; back off before trying to reopen.
                println!("  lost the board ({error}) — reconnecting");
            }
        }

        if sleep_backoff(&mut backoff, interrupted) {
            break;
        }
        check_audio(&capture, total_frames)?;
    }

    println!();
    let elapsed = started.elapsed();
    println!(
        "  {total_frames} frames in {:.1}s — {total_acks} acknowledgements discarded",
        elapsed.as_secs_f64()
    );
    if let Some((fps, target, achieved)) = last_rate {
        println!("  {achieved:.1} fps achieved against a {target} fps target");
        if fps != target {
            println!("  ended at {fps} fps — the board could not sustain the target");
        }
    }
    println!("  {total_blocks} blocks discarded after a stall, {total_resyncs} pacing resyncs");
    Ok(())
}

/// Turns Ctrl-C into a flag so the baseline can be restored before exiting.
/// A second Ctrl-C gives up and exits immediately.
fn install_interrupt_handler(what: &'static str) -> anyhow::Result<Arc<AtomicBool>> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || {
        if flag.swap(true, Ordering::SeqCst) {
            eprintln!("\naborting");
            std::process::exit(130);
        }
        eprintln!("\ninterrupted — {what}, Ctrl-C again to abort");
    })?;
    Ok(interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_line_is_stable() {
        assert_eq!(seed_line(42), "seed=42");
        assert_eq!(seed_line(u64::MAX), "seed=18446744073709551615");
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let mut backoff = Backoff::default();
        assert_eq!(backoff.delay(), Duration::from_secs(1));
        assert_eq!(backoff.delay(), Duration::from_secs(2));
        assert_eq!(backoff.delay(), Duration::from_secs(4));
        assert_eq!(backoff.delay(), Duration::from_secs(8));
        assert_eq!(backoff.delay(), Duration::from_secs(16));
        assert_eq!(backoff.delay(), Duration::from_secs(30), "capped");
        assert_eq!(backoff.delay(), Duration::from_secs(30), "stays capped");
    }

    /// A healthy audio stream for tests that do not care about audio.
    struct Healthy;
    impl AudioHealth for Healthy {
        fn error(&self) -> Option<String> {
            None
        }
    }

    /// A session over a fake link that opens cleanly then fails on the frame write.
    fn dead_port_session() -> Session<crate::testing::FakeLink> {
        let theme_reply = theme::encode_theme(&theme::uniform(4, Rgb::new(10, 20, 30)))
            .trim_start_matches("led.theme ")
            .to_string();
        let mut link = crate::testing::FakeLink::with_replies(&[
            ("led.mode", "1 \r\n"),
            ("led.theme", &theme_reply),
        ]);
        // The open sends three commands; the fourth — the first frame — fails.
        link.fail_after = Some(3);
        Session::open(link).expect("the fake opens")
    }

    fn test_visualizer() -> live::Live {
        live::Live::new(
            live::LiveConfig::default(),
            spectrum::BandPlan::standard(48_000, spectrum::FFT_SIZE),
            4,
            Instant::now(),
        )
    }

    #[test]
    fn run_frames_reports_a_lost_port() {
        let mut session = dead_port_session();
        let mut visualizer = test_visualizer();
        let (_sender, receiver) = mpsc::channel();
        let (detach, _acks) = run_frames(
            &mut session,
            &mut visualizer,
            &receiver,
            &Healthy,
            &AtomicBool::new(false),
        )
        .expect("a dead port is not an audio error");
        assert!(
            matches!(detach, Detach::Lost(_)),
            "a dead port must detach as Lost, got {detach:?}"
        );
    }

    #[test]
    fn run_frames_stops_when_interrupted() {
        let mut session = dead_port_session();
        let mut visualizer = test_visualizer();
        let (_sender, receiver) = mpsc::channel();
        let interrupted = AtomicBool::new(true);
        let (detach, _acks) = run_frames(
            &mut session,
            &mut visualizer,
            &receiver,
            &Healthy,
            &interrupted,
        )
        .unwrap();
        assert!(
            matches!(detach, Detach::Interrupted),
            "an already-set flag must detach as Interrupted"
        );
    }

    #[test]
    fn a_successful_connection_resets_the_backoff() {
        let mut backoff = Backoff::default();
        backoff.delay();
        backoff.delay();
        backoff.reset();
        assert_eq!(
            backoff.delay(),
            Duration::from_secs(1),
            "a reconnect must retry promptly, not at the capped delay"
        );
    }
}
