//! dygmarave — prove the LED path and measure the frame budget.
//!
//! Ticket 01's deliverable: find the board by USB identity, learn its LED count
//! from the board itself, hold a written frame long enough to prove no active
//! effect paints over it, measure how many full frames per second the link
//! sustains, and put the keyboard back exactly as it was found.

mod args;
mod audio;
mod bench;
mod level;
mod link;
mod session;
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

use audio::{AudioError, SourceKind};
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

    let interrupted = install_interrupt_handler()?;

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

/// Captures the output mix and prints a live level meter, so the audio path can
/// be checked before any DSP exists.
fn probe_audio(args: &args::Args, interrupted: &AtomicBool) -> Result<(), Box<dyn Error>> {
    let selection = audio::choose(args.source.as_deref())?;
    println!(
        "source: {selection}{}",
        if selection.follows_default_sink() {
            " — follows the default sink"
        } else {
            ""
        }
    );
    if selection.kind == SourceKind::Unknown {
        println!("  note: this device is not recognisable as a monitor; capturing it because you named it");
    }

    let (sender, receiver) = mpsc::channel();
    let capture = audio::start(&selection, sender)?;
    println!(
        "  {} channels at {} Hz, {:?}",
        capture.channels, capture.sample_rate, capture.sample_format
    );
    println!("  listening for {} s — play something…", args.seconds);

    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut loudest = 0.0f32;
    let mut held = vec![0.0f32; capture.channels as usize];
    let mut window: Vec<f32> = Vec::new();
    let mut last_draw = Instant::now();
    // A live meter redraws in place, but a journal or a pipe gets one line a
    // second instead of a wall of carriage returns.
    let interactive = std::io::stdout().is_terminal();
    let redraw_every = if interactive {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(1)
    };

    while Instant::now() < deadline && !interrupted.load(Ordering::SeqCst) {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(block) => window.extend_from_slice(&block.samples),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_draw.elapsed() < redraw_every {
            continue;
        }
        if let Some(levels) = level::measure(&window, capture.channels) {
            loudest = loudest.max(levels.loudest());
            for (index, peak) in levels.peak.iter().enumerate() {
                if let Some(slot) = held.get_mut(index) {
                    *slot = slot.max(*peak);
                }
            }
            if interactive {
                print!("\r{}    ", level::meter_line(&levels));
            } else {
                println!("{}", level::meter_line(&levels));
            }
            std::io::stdout().flush()?;
        }
        window.clear();
        last_draw = Instant::now();
    }
    if interactive {
        println!();
    }
    let stream_error = capture.take_error();

    if loudest <= 0.0 {
        if let Some(error) = stream_error {
            println!("  stream stopped: {error}");
        }
        return Err(AudioError::NoSignal {
            selection: Box::new(selection),
        }
        .into());
    }
    if let Some(error) = stream_error {
        println!("  note: the stream reported an error: {error}");
    }
    for (index, peak) in held.iter().enumerate() {
        println!(
            "  channel {} peak: {:.1} dBFS",
            index + 1,
            level::to_dbfs(*peak)
        );
    }
    println!(
        "  captured {} at {:.1} dBFS peak — this is the output mix, not the microphone",
        selection.display_name,
        level::to_dbfs(loudest)
    );
    Ok(())
}

/// Turns Ctrl-C into a flag so the baseline can be restored before exiting.
/// A second Ctrl-C gives up and exits immediately.
fn install_interrupt_handler() -> Result<Arc<AtomicBool>, Box<dyn Error>> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || {
        if flag.swap(true, Ordering::SeqCst) {
            eprintln!("\naborting without restore");
            std::process::exit(130);
        }
        eprintln!("\ninterrupted — restoring the keyboard, Ctrl-C again to abort");
    })?;
    Ok(interrupted)
}
