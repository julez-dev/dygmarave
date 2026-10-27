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
mod bench;
mod layout;
mod level;
mod link;
mod live;
mod render;
mod session;
mod spectrum;
mod theme;

#[cfg(test)]
mod testing;

use std::error::Error;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use audio::{verdict, AudioError, SourceKind, Verdict};
use link::DygmaLink;
use session::Session;
use theme::{Frame, Rgb};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dygmarave: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = args::parse(std::env::args().skip(1))?;
    if args.help {
        println!("{}", args::USAGE);
        return Ok(());
    }
    if args.live && args.probe_audio {
        return Err("--live and --probe-audio are mutually exclusive;                     the first drives the board, the second only prints a meter"
            .into());
    }

    let interrupted = install_interrupt_handler(if args.probe_audio {
        "stopping"
    } else {
        "restoring the keyboard"
    })?;

    if args.columns || args.rows || args.gradient || args.thumb_pattern {
        return paint_pattern(&args, &interrupted);
    }
    if args.probe_audio {
        return probe_audio(&args, &interrupted);
    }
    if args.live {
        return live_visualizer(&args, &interrupted);
    }

    let device = match &args.port {
        Some(port) => DygmaLink::from_port(port)?,
        None => DygmaLink::discover()?,
    };
    println!("port: {}", device.port_name());

    let mut session = Session::open(device)?;
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
fn paint_pattern(args: &args::Args, interrupted: &AtomicBool) -> Result<(), Box<dyn Error>> {
    let device = match &args.port {
        Some(port) => DygmaLink::from_port(port)?,
        None => DygmaLink::discover()?,
    };
    println!("port: {}", device.port_name());
    let mut session = Session::open(device)?;
    let count = session.led_count();
    println!(
        "LED count: {count}; the grid claims {} key LEDs across {} rows x {} columns",
        layout::key_leds().len(),
        layout::ROWS,
        layout::COLUMNS
    );

    if args.gradient {
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

    let colors = if args.gradient {
        layout::gradient(count)
    } else if args.thumb_pattern {
        println!(
            "\n  guessed thumb clusters — left magenta, right cyan; the dim grey is the rest of the underglow"
        );
        layout::thumb_pattern(count)
    } else if args.rows {
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
        if args.gradient {
            "Red = lowest band at the inner edge, blue = highest at the outer edges."
        } else if args.thumb_pattern {
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
    fn new(args: &args::Args, capture: &audio::Capture) -> Self {
        if args.probe_spectrum {
            let plan = spectrum::BandPlan::standard(capture.sample_rate, spectrum::FFT_SIZE);
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
fn probe_audio(args: &args::Args, interrupted: &AtomicBool) -> Result<(), Box<dyn Error>> {
    let selection = audio::choose(args.source.as_deref())?;
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
    let capture = audio::start(&selection, sender)?;
    println!(
        "  {} channels at {} Hz, {:?}",
        capture.channels, capture.sample_rate, capture.sample_format
    );
    let mut probe = Probe::new(args, &capture);
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

/// Runs the visualizer until interrupted: audio in, frames out, steady pace.
///
/// The audio source is resolved *before* the board is touched, so a failed
/// capture leaves the keyboard alone rather than flashing it and restoring.
fn live_visualizer(args: &args::Args, interrupted: &AtomicBool) -> Result<(), Box<dyn Error>> {
    let selection = audio::choose(args.source.as_deref())?;
    println!("source: {selection}");

    let (sender, receiver) = mpsc::channel();
    let capture = audio::start(&selection, sender)?;
    println!(
        "  {} channels at {} Hz — drive the board until Ctrl-C",
        capture.channels, capture.sample_rate
    );

    let device = match &args.port {
        Some(port) => DygmaLink::from_port(port)?,
        None => DygmaLink::discover()?,
    };
    let port_name = device.port_name().to_string();
    let mut session = Session::open(device)?;
    let count = session.led_count();

    let plan = spectrum::BandPlan::standard(capture.sample_rate, spectrum::FFT_SIZE);
    let config = live::LiveConfig {
        fps: args.fps,
        channels: capture.channels as usize,
    };
    let mut visualizer = live::Live::new(config, plan, count, Instant::now());
    println!(
        "  port {port_name} — {count} LEDs, {} fps, {} bands",
        visualizer.fps(),
        visualizer.bands()
    );

    let started = Instant::now();
    let mut drain = Cadence::new(DRAIN_INTERVAL, started);
    let mut probe = Cadence::new(PROBE_INTERVAL, started);
    let mut report = Cadence::new(REPORT_INTERVAL, started);
    let mut acks = 0u64;
    while !interrupted.load(Ordering::SeqCst) {
        let now = Instant::now();
        // Drain every block in order so the analyzer's window stays contiguous;
        // only a multi-second stall trips the cap and drops the oldest whole
        // blocks. Draining to just the newest block would splice two moments of
        // audio together and click, which reads as strobing at the frame rate.
        let (latest, dropped) = live::Live::drain(&receiver);
        let frame = visualizer.step(latest.as_ref(), now, dropped);
        session.write_frame(&frame)?;

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
            acks += session.drain_acks()? as u64;
        }
        // Prove the board is still answering, not just accepting writes. The
        // query drains first, so it also clears acknowledgements as a side effect.
        if probe.due(now) {
            session.health_check()?;
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

        if let Some(error) = capture.take_error() {
            // A mid-set stream failure is not "no signal was ever heard" — that
            // error means every sample was zero. Report the actual failure and
            // let Session's Drop put the board back on the way out.
            return Err(format!(
                "audio stream stopped mid-set after {} frames: {error}",
                visualizer.frames()
            )
            .into());
        }
        session::sleep_unless_interrupted(wait, interrupted);
    }

    println!();
    let elapsed = started.elapsed();
    println!(
        "  {} frames in {:.1}s — {:.1} fps achieved against a {} fps target",
        visualizer.frames(),
        elapsed.as_secs_f64(),
        visualizer.achieved_fps(Instant::now()),
        visualizer.target_fps()
    );
    if visualizer.fps() != visualizer.target_fps() {
        println!(
            "  ended at {} fps — the board could not sustain the target",
            visualizer.fps()
        );
    }
    println!(
        "  {} blocks discarded after a stall, {acks} acknowledgements discarded, {} pacing resyncs",
        visualizer.dropped_blocks(),
        visualizer.resyncs()
    );
    session.restore()?;
    println!(
        "  restored LED mode {} ({})",
        session.baseline().mode.code(),
        session.baseline().mode.name()
    );
    Ok(())
}

/// Turns Ctrl-C into a flag so the baseline can be restored before exiting.
/// A second Ctrl-C gives up and exits immediately.
fn install_interrupt_handler(what: &'static str) -> Result<Arc<AtomicBool>, Box<dyn Error>> {
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
