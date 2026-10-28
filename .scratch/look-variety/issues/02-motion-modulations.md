# 02: Motion modulations

**What to build:** The additive motion axis: `none`, `sweep`, `sweep-vertical`,
`sparkle`, `breathe`, `ripple`, `orbit`, `rain`. A motion modulates the matrix
after the palette and layout have run; it never touches the thumb clusters, and
`intensity = 0` is exactly identity. Motion state lives in one struct driven by a
clock and by the existing beat/level signals.

**Blocked by:** 01

**Status:** ready-for-agent

## Acceptance criteria

- [ ] `Motion` names all eight values including `none`; name parsing is shared
      with the config/CLI layer in ticket 04.
- [ ] `compose` accepts an active motion plus `motion_intensity` and
      `motion_speed`, and applies it to matrix LEDs only. Thumbs keep the existing
      palette-level + white-flash behaviour.
- [ ] `intensity = 0` returns the unmoved frame byte-for-byte for every motion.
- [ ] `sweep` and `sweep-vertical` move a smooth crest with the documented period;
      brightness never drops below 0.3 and never exceeds the palette colour.
- [ ] `breathe` follows the bass/thumb level, not a free-running clock.
- [ ] `sparkle` only adds glints after a beat, decays each glint in ~120 ms, and
      scales its count with intensity.
- [ ] `ripple` only starts on a beat and expands to the half's extent in ~600 ms.
- [ ] `orbit` is periodic and mirrored across halves; `rain` is deterministic for
      a given seed and advances with the clock.
- [ ] Motion event state (`sparkle`, `ripple`, `rain`) has a `reset()` used by
      ticket 03 on a switch. Periodic phase is not reset by the fade or by
      `reset()`; it reads one continuous clock.
- [ ] No per-frame allocation in the motion path; modulation is arithmetic over
      the existing colour buffer.
- [ ] Tests: identity at zero intensity, periodicity, beat gating, reset
      behaviour, determinism, clamping, and thumbs unaffected.
- [ ] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

## Notes

- Use the existing `Dynamics` signals: per-channel `thumb_level`, `is_flashing`,
  and the band levels. Do not add new audio analysis.
- A motion is a modulation, not a renderer: the layout's spectrum must stay
  recognisable underneath. Brighten rather than blank wherever possible.
- The motion clock should be passed in (wall-clock derived) so tests can drive it
  deterministically. Do not read `Instant::now()` inside the motion functions.
- Keep modulation arithmetic in one place so the clamp is impossible to forget.
