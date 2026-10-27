//! dygmarave — prove the LED path and measure the frame budget.
//!
//! Ticket 01's deliverable: find the board by USB identity, learn its LED count
//! from the board itself, hold a written frame long enough to prove no active
//! effect paints over it, measure how many full frames per second the link
//! sustains, and put the keyboard back exactly as it was found.

mod args;
mod bench;
mod link;
mod session;
mod theme;

#[cfg(test)]
mod testing;

use std::error::Error;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use link::DygmaLink;
use session::Session;

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

    let device = match &args.port {
        Some(port) => DygmaLink::from_port(port)?,
        None => DygmaLink::discover()?,
    };
    println!("port: {}", device.port_name());

    let mut session = Session::open(device)?;
    println!("firmware: {}", session.firmware()?);
    let count = session.led_count();
    println!(
        "LED count: {count} (reported by the board, not assumed)"
    );
    println!(
        "baseline: LED mode {} ({}), {} distinct colours",
        session.baseline().mode,
        mode_name(session.baseline().mode),
        distinct(&session.baseline().theme)
    );
    println!("switched to static LED mode ({})", session::STATIC_MODE);

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
    let frame = theme::encode_theme(&theme::uniform(count, args.colour));
    println!(
        "\nmeasuring {} full frames of {} bytes, fire-and-forget…",
        args.frames, frame.len() + 1
    );
    let result = bench::measure(&mut session, &frame, args.frames, &interrupted)?;
    if interrupted.load(Ordering::SeqCst) {
        println!("interrupted after {} frames", result.frames);
    }
    println!(
        "  {} frames in {} ms",
        result.frames,
        result.elapsed.as_millis()
    );
    println!("  {:.1} fps, {:.2} ms per frame", result.fps(), result.ms_per_frame());
    println!("  {:.0} KiB/s of LED data", result.kib_per_second());
    println!(
        "  {} acknowledgements discarded (the board acks every frame)",
        result.acks_drained
    );

    // Put the board back the way it was found.
    session.restore()?;
    println!(
        "\nrestored baseline theme and LED mode {} ({})",
        session.baseline().mode,
        mode_name(session.baseline().mode)
    );
    Ok(())
}

/// The LED modes this firmware accepts, named for the log.
fn mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "static",
        1 => "rainbow",
        2 => "cycle",
        3 => "stalker",
        _ => "unknown",
    }
}

fn distinct(colors: &[theme::Rgb]) -> usize {
    let mut unique = colors.to_vec();
    unique.sort_by_key(|color| (color.r, color.g, color.b));
    unique.dedup();
    unique.len()
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
