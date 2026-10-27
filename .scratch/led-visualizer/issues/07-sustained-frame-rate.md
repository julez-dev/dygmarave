# 07: Sustained frame rate — fire-and-forget writes and adaptive pacing

**What to build:** Making the rate robust. Frames are written without waiting for each terminator, a periodic health probe notices a port that has stopped accepting writes, and the pacing loop lowers the frame rate when the wire cannot keep up rather than dropping bands or letting a backlog build. The daemon reports the rate it is actually achieving, so a slow board is visible rather than mysterious.

**Blocked by:** 05

**Status:** done

- [x] Frames are written fire-and-forget, with no per-frame wait for the response terminator.
- [x] Acknowledgements are drained unconditionally in the render loop, at a cadence that keeps the port from stalling over an hour-long show (ticket 01 drains every 64 frames as a stopgap).
- [x] A port that stops accepting writes is detected by the health probe rather than silently dropping frames forever.
- [x] When the board cannot sustain the target rate, fps degrades and recovers, and spectral fidelity is never reduced.
- [x] The achieved rate is reported so it can be compared against the target.

## Not a reason to raise the frame rate

A layer change repaints the whole board from that layer's colormap, which wipes
whatever frame was showing. There is no host-side prevention: the Focus API has
no command to disable the colormap, lock the layer, or freeze the LED buffer, and
`led.mode` only chooses which effect owns the LEDs (0 is the colormap, 1–3
repaint continuously, 4+ is rejected by the firmware).

The maintainer chose to **accept the one-frame blink** rather than raise the
frame rate to shorten it (30 fps → ~33 ms; the measured ceiling of ~70 fps would
give ~14 ms but cost typing latency and CPU). So this ticket's job is the target
rate and the adaptive fallback, not chasing the flash.

## Review round

Code review returned **OK with notes** — all five criteria implemented. Folded:

- **P1 — duplicated mode-reply parsing.** `Session::open` and `Session::health_check`
  each parsed the `led.mode` reply. Extracted to `mode_from_reply`.
- **P2 — duplicated fps→period conversion.** Three copies collapsed into `period_of`.
- **P2 — speculative clamps in `RateControl::new`.** Dropped guards against
  impossible configs; kept the one that matters (`--fps 1..4` would otherwise
  "degrade" upward past the target).
- **P2 — cadence duplication in the live loop.** Three `last_*` + interval pairs
  collapsed into a `Cadence` helper at module scope.
- **P2 — duplicate frame/started accounting.** The loop reports through
  `Live::frames()` and `Live::recent_fps()`; the cumulative figure stays in the exit
  summary.
- **P2 — the achieved rate masked recent slowdowns.** A 10 s report of a cumulative
  average hides a recent dip, so the periodic report prints the last window's rate.
- **P2 — spec misattribution.** The floor/window/hysteresis numbers are this module's
  choices, not the spec's; the comment now says so and this ticket records them.
- **P2 — health probe and EEPROM.** A read-only `led.mode` query does not write
  EEPROM; the doc comment says so and notes the 5 s reply timeout as the price of a
  definitive answer.

Skipped as judgement calls: renaming `Live::fps()` to `current_fps()`.

## Open verification

- **Ack-drain cadence over a long show.** The 250 ms wall-clock cadence is reasoned
  from the measured ~1.2 KB/s acknowledgement volume and covered by the fake link's
  drain tests, but the real proof — an hour-long soak with no port stall — needs the
  physical board. This is the one criterion not yet demonstrated end to end.
