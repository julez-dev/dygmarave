# 01: Layout permutations and palette ramps

**What to build:** The eight spatial layouts and the eight colour palettes, wired
into `render::compose` behind fixed names. After this ticket the board can be
configured in code to show any layout with any palette, but still uses
`snake`/`ember` by default and has no motion or randomisation.

**Blocked by:** none

**Status:** ready-for-agent

## Acceptance criteria

- [ ] `layout::Layout` names all eight layouts: `snake`, `vertical-snake`, `rows`,
      `columns`, `rings`, `diagonal`, `spiral`, `scatter`. Names parse from and
      render to the config spelling.
- [ ] `layout::band_leds_for(Layout, Half)` returns one LED per band for the
      active layout. `Layout::Snake` reproduces the current `band_leds` output
      exactly (byte-identical LED vectors on both halves).
- [ ] The other geometric layouts are deterministic, mirrored across halves, and
      defined exactly as the spec describes (`vertical-snake`, `rows`, `columns`,
      `rings`, `diagonal`, `spiral`).
- [ ] `scatter` builds a Fisher–Yates permutation of the half's present keys from
      a caller-supplied `fastrand::Rng`; the active permutation is frozen for the
      turn rather than regenerated per frame. The caller-facing shape supports
      ticket 03 drawing a fresh one per selection.
- [ ] Every layout is a permutation: no duplicated band, no dropped present key,
      no band on a hole, no band on a thumb or underglow LED. Tests cover 26 keys
      left, 27 right, and the mirrored counterpart of every left key except the
      one documented in ticket 04's notes.
- [ ] `palette::Palette` names all eight ramps and holds sorted `(position, Rgb)`
      stops. `palette::sample(palette, level, floor)` returns black below `floor`
      and otherwise interpolates the stops.
- [ ] `ember` reproduces the current `render::ramp_with_floor` output exactly
      (regression test over sampled levels, including the floor boundary).
- [ ] `render::compose` takes the layout and palette (a small `Look`/selection
      value is fine) instead of hard-coding `band_leds` and `ramp_with_floor`. The
      default selection produces the same frame as v0.1.
- [ ] Thumb behaviour is unchanged: active palette for the level glow, white beat
      flash, no motion.
- [ ] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

## Notes

- The physical map lives in `src/layout.rs`; the normalised half coordinates are
  `row` 0–3 and `depth` 0–6 inner→outer, with left holes at `(1,0)`/`(2,0)` and
  right hole at `(3,0)`.
- `rings` ordering: `4·depth² + (2·row−3)²`, ties by row then depth.
- `diagonal`: `row + depth`, ties by row.
- `spiral`: write the path explicitly rather than deriving a clever formula; the
  acceptance test is "visits every present key once, looks like a spiral".
- `spectrum` is intentionally non-monotonic in luminance. Do not add a global
  monotonic-brightness test for every palette; assert it only for the palettes
  whose stops are designed that way.
- Probe rendering: it is useful for this ticket to add static layout probes (a
  gradient painted through each ordering) so ticket 04 can photograph them.
  Keep them behind the existing probe path, not in the live loop.
