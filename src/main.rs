//! dygmarave — audio-reactive LEDs on a Dygma Raise.
//!
//! Two probes exist so far, one per half of the eventual pipeline:
//!
//! * the default `LED path` finds the board by USB identity, learns its LED count
//!   from the board itself, holds a written frame long enough to prove no active
//!   effect paints over it, measures full-frame throughput, and restores the
//!   keyboard exactly as it was found (ticket 01);
//! * `--probe-audio` captures the output mix and prints a live level meter,
//!   refusing microphones (ticket 02).

mod args;
mod audio;
mod bench;
mod level;
mod link;
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

    let interrupted = install_interrupt_handler(if args.probe_audio {
        "stopping"
    } else {
        "restoring the keyboard"
    })?;

    if args.probe_audio {
        return probe_audio(&args, &interrupted);
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

/// What a probe draws while it runs.
enum Probe {
    Level {
        window: Vec<f32>,
        held: Vec<f32>,
        loudest: f32,
    },
    Spectrum {
        analyzer: Box<spectrum::Analyzer>,
        /// Loudest band seen per channel, and its amplitude.
        loudest: Vec<(usize, f32)>,
        amplitude: f32,
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
                amplitude: 0.0,
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
                amplitude,
            } => {
                let spectrum = analyzer.spectrum()?;
                for (channel, slot) in loudest.iter_mut().enumerate() {
                    let (band, level) = spectrum.loudest_band(channel);
                    *amplitude = amplitude.max(level);
                    if level >= slot.1 {
                        *slot = (band, level);
                    }
                }
                Some(render_bands(&spectrum))
            }
        }
    }

    fn loudest(&self) -> f32 {
        match self {
            Probe::Level { loudest, .. } => *loudest,
            Probe::Spectrum { amplitude, .. } => *amplitude,
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

/// Captures the output mix and prints a live readout, so the audio path can be
/// checked before any DSP exists.
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
