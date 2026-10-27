# 05: Live visualizer — spectrum to board

**What to build:** The first ticket where the product exists: music playing on the computer drives the board, continuously. Band energies pass through the colour ramp, instant attack with slow decay, per-band AGC and a silence gate before being written as full frames at a steady pace. Quiet passages go black rather than showing amplified noise, and quiet masters still light the board.

**Blocked by:** 03, 04

**Status:** done

- [x] Playing music produces a moving spectrum on the board that tracks the audio, end to end.
- [x] Transients pop immediately and fade smoothly afterwards, with no strobing at the frame rate.
- [x] Silence, and audio below the gate, leaves the board dark instead of showing amplified noise.
- [x] A quietly mastered track still fills the board, and a loud one does not clip into a wall of white.
- [x] Frame pacing is steady with no unbounded backlog; latency does not drift away from the music over minutes.

## Review round

Two-axis review of the first cut. Both verdicts were "OK with notes" — all five
criteria held, no P0/P1 — but both independently caught the same two P2s, plus a
set of P3s. Everything below was folded before the merge commit.

- **P2 — a stalled capture left the board frozen lit.** When the sink suspends it
  delivers no buffers *and* reports no error, so `Analyzer::spectrum()` kept
  re-computing the retained window and the picture stayed on screen while nothing
  played. `Live` now tracks when audio last arrived and, after two frames of
  silence, feeds a zeroed spectrum so the gate closes and the picture fades out.
  A new test (`a_tone_fades_to_black_when_the_stream_stops`) covers it. This also
  needed a floor in `ramp`: a decaying level approaches zero without arriving, so
  without one the board would glow faintly red forever instead of going dark.
- **P2 — `dt` was the nominal period, not the time that elapsed.** After a stall
  or a slow frame the fade was applied for less wall-clock time than had passed,
  contradicting the documented frame-rate independence. `Live` now measures
  `now - last_step` (clamped to two periods) and uses that.
- **P2 — the loop comment described the opposite of the code.** It said "take the
  newest block and throw the rest away", which is the splice-inducing design the
  module doc argues against. Corrected, so a future reader cannot "fix" `drain`
  into reintroducing the click.
- **P2 — mid-set audio failure was reported as `NoSignal`.** That error means
  "every sample was zero", not "the stream died"; it also passed the frame count
  as a block count. It now returns the actual stream error and the frame count.
- **P2 — `ramp` re-derived each segment's start from the colour pair**, so a
  boundary moved in the `if` could silently disagree with the `match`. The branch
  now returns start and width together.
- **P2/P3 — weak or lying tests.** The compose test asserted a *count* of lit
  LEDs, so a bug lighting the wrong key passed; it now checks every LED against
  the band map. The backlog-cap test used identical samples, so dropping the
  *newest* would have passed; it now uses distinct values and checks the survivor
  starts at the first block after the drop. The pacing test only lower-bounded
  elapsed time and now also bounds it above. The dead first half of the
  frame-rate test was deleted. `drain_acks` had no test; it has one.
- **P3 — `--fps` accepted 0 and absurd values.** 0 silently became 1 fps, and a
  huge value made the period vanish into a write storm. Validated to 1–240.
- **P3 — `--live --probe-audio` silently ran the probe.** Now rejected as
  mutually exclusive.
- **P3 — `sleep_unless_interrupted` existed twice**, with different slice sizes.
  There is now one, shared.
- **P3 — unattributed tuning.** `target = 0.7` and `max_gain = 100.0` are now in
  the spec's config sketch with their reasoning.
