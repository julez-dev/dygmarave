# 02: Motion modulations

**What to build:** The additive motion axis: `none`, `sweep`, `sweep-vertical`,
`sparkle`, `breathe`, `ripple`, `orbit`, `rain`. A motion modulates the matrix
after the palette and layout have run; it never touches the thumb clusters, and
`intensity = 0` is exactly identity. Motion state lives in one struct driven by a
clock and by the existing beat/level signals.

**Blocked by:** 01

**Status:** done

## Acceptance criteria

- [x] `Motion` names all eight values including `none`; name parsing is shared
      with the config/CLI layer in ticket 04.
- [x] `compose` accepts an active motion plus `motion_intensity` and
      `motion_speed`, and applies it to matrix LEDs only. Thumbs keep the existing
      palette-level + white-flash behaviour.
- [x] `intensity = 0` returns the unmoved frame byte-for-byte for every motion.
- [x] `sweep` and `sweep-vertical` move a smooth crest with the documented period;
      brightness never drops below 0.3 and never exceeds the palette colour.
- [x] `breathe` follows the bass/thumb level, not a free-running clock.
- [x] `sparkle` only adds glints after a beat, decays each glint in ~120 ms, and
      scales its count with intensity.
- [x] `ripple` only starts on a beat and expands to the half's extent in ~600 ms.
- [x] `orbit` is periodic and mirrored across halves; `rain` is deterministic for
      a given seed and advances with the clock.
- [x] Motion event state (`sparkle`, `ripple`, `rain`) has a `reset()` used by
      ticket 03 on a switch. Periodic phase is not reset by the fade or by
      `reset()`; it reads one continuous clock.
- [x] No per-frame allocation in the motion path; modulation is arithmetic over
      the existing colour buffer.
- [x] Tests: identity at zero intensity, periodicity, beat gating, reset
      behaviour, determinism, clamping, and thumbs unaffected.
- [x] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

## Notes

- Use the existing `Dynamics` signals: per-channel `thumb_level`, `is_flashing`,
  and the band levels. Do not add new audio analysis.
- A motion is a modulation, not a renderer: the layout's spectrum must stay
  recognisable underneath. Brighten rather than blank wherever possible.
- The motion clock should be passed in (wall-clock derived) so tests can drive it
  deterministically. Do not read `Instant::now()` inside the motion functions.
- Keep modulation arithmetic in one place so the clamp is impossible to forget.

## Implementer handoff

Implemented on branch `look-variety`; ticket 03 is untouched.

- **State vs selection.** `MotionState` is the explicit motion selection *and*
  state: it owns `MotionSettings { motion, intensity, speed }`, the seed/RNG, one
  continuous `elapsed` clock, and fixed event buffers. `Look` stays the resolved
  layout/palette selection and `compose(dynamics, count, look, &motion)` takes the
  state separately. That is the "equivalent explicit selection value" the ticket
  allows: it avoids duplicating motion settings between a selection value and the
  live state, and makes the crossfade contract obvious — compose is read-only over
  `&MotionState`.
- **Advance once, render many.** `MotionState::advance(now, dynamics)` owns the
  clock, glint/ripple ageing, beat edge detection (`Dynamics::beat_count`) and
  spawn decisions. `modulate` is pure over `&self`. `reset_events` clears
  glints/ripples and re-draws rain from the continuing RNG, but never touches
  `elapsed`. A live switch uses `reset_events_at(dynamics)`, which also snapshots
  the current beat counters; a beat after the switch and before that frame's
  advance still fires, while a beat from before the switch cannot fire late.
- **Fixed buffers.** 16 glint slots and 2 ripple slots per half, 4 rain droplets
  per half, all fixed-size arrays. No motion-path allocation; `advance` and
  `modulate` only do arithmetic.
- **Geometry.** `layout::Cell` is now public with row/depth/index accessors, and
  `layout::band_cells_for` returns each present band LED with its normalised cell.
  `compose` uses that so motion does not re-derive geometry or allocate another
  layout vector for the motion pass.
- **Effects.** Sweep uses a cosine crest, `period = 3 s / speed`, floor `0.3`.
  Breathe uses `thumb_level`. Sparkle spawns 1..9 glints by intensity with an
  exponential ~120 ms decay. Ripple is a fixed 600 ms regardless of speed and
  expands inner→outer by depth. Orbit is a soft white blend on a 4 s/speed circle
  in normalised coordinates, so both halves mirror. Rain has four droplets per
  half falling at `3 rows/s * speed`; reset draws a new pattern from the continuing
  RNG.
- **Thumbs.** Matrix keys are modulated during `compose`'s band loop; thumbs are
  painted afterwards from the unchanged palette-level/white-flash path. Thumbs
  keep `Dynamics::is_flashing`; only event motions use the counter.
- **Live.** `Live` owns a default `Look` and `MotionState`, calls
  `motion.advance(now, &dynamics)` once per frame in `step`, then composes. Ticket
  03 can replace the acquisition of `now` without moving state.

Validation run locally: `cargo test` (227 tests), `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`.

## Review fixes (ticket 02)

- `Dynamics` now has a monotonic `beat_count(channel)` incremented exactly on the
  existing rising edge. `is_flashing` remains the thumb flash only. `MotionState`
  tracks the last seen count per half and spawns once per missed count, so two
  beats inside one flash window spawn twice and a 0.2 s frame cannot miss a beat.
- `reset_events()` remains the construction/startup path and deliberately
  suppresses the first beat until an advance. Live switches use
  `reset_events_at(&Dynamics)` (or `set_motion_at`/`set_settings_at`), which clears
  events and snapshots the current counters immediately. A beat that arrives after
  the switch but before the frame's advance is therefore not swallowed.
- Ripple lifetime is fixed at 600 ms; sweep/orbit/rain remain speed-scaled.
- Resets no longer re-seed the RNG: rain and sparkle patterns continue from the
  live stream. Two identical seeds still reproduce the same reset sequence.
- Geometry helpers are `pub(crate)`, `modulate` takes a `Half`, and ripple also
  early-returns at zero intensity.

## Reset ordering fix (ticket 02 review)

- Added `MotionState::reset_events_at(&Dynamics)`, `set_motion_at` and
  `set_settings_at`. They clear event state and snapshot `Dynamics::beat_count`
  at the switch, so the switch path has no window where a real post-switch beat
  is lost. `reset_events()` stays for construction/startup and documents that it
  suppresses the first beat until the next advance.
- Tests pin the exact order beat → advance → `reset_events_at` → beat (no
  intermediate advance) → advance, and prove a pre-switch beat cannot fire late.
  The overlapping-beats test now uses 10 ms dynamics steps and asserts the flash
  is still active after each beat, so it fails if gating reverts to flash edges.


## Controller acceptance

Review round 1 found one blocker: `sparkle`/`ripple` were gated on
`Dynamics::is_flashing`, which is already false at the 5 fps floor and merges
overlapping beats. Fixed in `7acda62` with a monotonic per-channel
`Dynamics::beat_count`; `MotionState` now consumes counter edges. `6d18022`
snapshots the counter at reset time so a switch cannot swallow the next beat.

Review round 2 verdict: **ACCEPT** — no blockers. Carry-over to ticket 03: the
plain `set_motion`/`set_settings` setters still use the startup reset and would
reintroduce the swallowed-beat bug if called live. Ticket 03 must use the `_at`
variants on live switches, and should make the plain setters test-only or
otherwise unrepresentable there. The unresolved API trap is not live today.
