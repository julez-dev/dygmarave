# 02: Motion modulations

**What to build:** The additive motion axis: `none`, `sweep`, `sweep-vertical`,
`sparkle`, `breathe`, `ripple`, `orbit`, `rain`. A motion modulates the matrix
after the palette and layout have run; it never touches the thumb clusters, and
`intensity = 0` is exactly identity. Motion state lives in one struct driven by a
clock and by the existing beat/level signals.

**Blocked by:** 01

**Status:** ready-for-agent

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
  clock, glint/ripple ageing, beat rising-edge detection (`Dynamics::is_flashing`)
  and spawn decisions. `modulate` is pure over `&self`. `reset_events` clears
  glints/ripples and re-draws deterministic rain, but never touches `elapsed`.
  A reset also suppresses a beat trigger from a flash already in progress.
- **Fixed buffers.** 16 glint slots and 2 ripple slots per half, 4 rain droplets
  per half, all fixed-size arrays. No motion-path allocation; `advance` and
  `modulate` only do arithmetic.
- **Geometry.** `layout::Cell` is now public with row/depth/index accessors, and
  `layout::band_cells_for` returns each present band LED with its normalised cell.
  `compose` uses that so motion does not re-derive geometry or allocate another
  layout vector for the motion pass.
- **Effects.** Sweep uses a cosine crest, `period = 3 s / speed`, floor `0.3`.
  Breathe uses `thumb_level`. Sparkle spawns 1..9 glints by intensity with an
  exponential ~120 ms decay. Ripple scales its 600 ms life by speed and expands
  inner→outer by depth. Orbit is a soft white blend on a 4 s/speed circle in
  normalised coordinates, so both halves mirror. Rain has four seeded droplets per
  half falling at `3 rows/s * speed`; reset re-draws the seeded pattern.
- **Thumbs.** Matrix keys are modulated during `compose`'s band loop; thumbs are
  painted afterwards from the unchanged palette-level/white-flash path.
- **Live.** `Live` owns a default `Look` and `MotionState`, calls
  `motion.advance(now, &dynamics)` once per frame in `step`, then composes. Ticket
  03 can replace the acquisition of `now` without moving state.

Validation run locally: `cargo test` (214 tests), `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`.
