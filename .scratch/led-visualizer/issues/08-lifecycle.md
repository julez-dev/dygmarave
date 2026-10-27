# 08: Lifecycle — reconnect, Bazecor pause, baseline restore, systemd unit

**What to build:** Surviving the real world of a desk. Unplugging the board mid-song does not kill the daemon: it keeps the audio pipeline alive, retries, and resumes with the correct LED count and mode when the board returns. Launching Bazecor pauses writes instead of corrupting the command stream, and resuming once it exits. A clean shutdown restores the baseline theme and the original LED mode, and a systemd user unit runs the whole thing at login without its restart policy fighting the daemon's own retry loop.

**Blocked by:** 01, 05

**Status:** done

- [x] Unplugging and replugging mid-show resumes rendering without a restart, with the LED count and mode re-established.
- [x] Bazecor running pauses writes; quitting it resumes them.
- [x] SIGTERM and clean exit restore the baseline theme and the mode that was in use before startup.
- [x] A systemd user unit starts the daemon at login, and systemd does not restart it in a loop while the daemon is retrying internally.

## Review round

Two-axis review: **Standards — OK with notes**; **Spec — BLOCK** on one P1. Both
were folded:

- **P1 — the pre-startup baseline was discarded on every reconnect.** `Session::open`
  re-read `led.mode`/`led.theme` on every open, so after a Bazecor pause or a replug
  the board (still in static mode showing our last frame) became the "baseline", and
  exit wrote our own frame back instead of the user's original. Fixed: the daemon
  captures the first baseline and passes it to `Session::open_with` on every
  reconnect, so `restore` always writes the original theme and mode. Tested by
  `reopening_with_a_known_baseline_skips_the_read`.
- **P2 — the outer loop was untested.** `run_frames` is now generic over `Link` and
  takes an `AudioHealth` trait, so `FakeLink` plus a healthy/failing audio stub
  drive it. `run_frames_reports_a_lost_port` and `run_frames_stops_when_interrupted`
  cover the detach paths.
- **P2 — audio failure invisible during a pause.** The pause and backoff paths now
  poll the audio error slot, so a stream that dies while Bazecor holds the port is
  reported as fatal rather than silently ignored.
- **P2 — duplicate Bazecor message.** A `Detach::Paused` printed "Bazecor took the
  port" and then the top-of-loop printed "Bazecor is running"; the redundant line is
  gone.
- **P2 — the SIGTERM restore path.** Confirmed: `ctrlc`'s `termination` feature is
  enabled, the interrupted path restores and exits 0, and a restore failure is
  reported but still exits cleanly so `Restart=on-failure` does not loop.
- **P2 — the pause loop must not grow memory.** During a Bazecor pause or a retry
  backoff the audio backlog is drained and discarded; the spec's cap-32 only applies
  when `Live::drain` runs, which it does not while paused.
- **P2 — systemd unit ordering and restart storms.** `After=sound.target` is a
  system unit and inert in a user manager; replaced with `After=pipewire.service
  wireplumber.service`, plus `StartLimitIntervalSec=600`/`StartLimitBurst=5` so a
  genuine start failure stops instead of looping. Install instructions are in the
  unit.
- **P3 — the Bazecor poll interval.** A one-second scan keeps the corruption window
  small without reading `/proc` every frame; documented as best-effort, since no OS
  lock exists to make it exact.
- **Skipped as judgement calls:** the `last_rate` tuple and the `open_session` pair
  could each become a small struct; neither is worth the churn here.

## Open verification

- **Bazecor pause and reconnect on the real board.** The process scan and the
  outer-loop reconnect are covered by the fake link and the unit tests, but the
  proof — launching Bazecor mid-show and pulling the USB cable — needs the physical
  board. The daemon's logic is exercised end to end only against the fake link.
