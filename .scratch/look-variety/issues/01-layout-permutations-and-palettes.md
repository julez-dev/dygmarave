# 01: Layout permutations and palette ramps

**What to build:** The eight spatial layouts and the eight colour palettes, wired
into `render::compose` behind fixed names. After this ticket the board can be
configured in code to show any layout with any palette, but still uses
`snake`/`ember` by default and has no motion or randomisation.

**Blocked by:** none

**Status:** ready-for-agent

## Acceptance criteria

- [x] `layout::LayoutKind` names all eight layouts: `snake`, `vertical-snake`,
      `rows`, `columns`, `rings`, `diagonal`, `spiral`, `scatter`. Names parse from
      and render to the config spelling; resolving a kind is the only way to get a
      concrete `layout::Layout`.
- [x] `layout::band_leds_for(Layout, Half)` returns one LED per band for the
      active layout. `Layout::Snake` reproduces the current `band_leds` output
      exactly (byte-identical LED vectors on both halves).
- [x] The other geometric layouts are deterministic, mirrored across halves, and
      defined exactly as the spec describes (`vertical-snake`, `rows`, `columns`,
      `rings`, `diagonal`, `spiral`).
- [x] `scatter` builds a Fisher–Yates permutation of the full 4×7 normalised
      rectangle from a caller-supplied `fastrand::Rng`, then each half filters its
      own holes. The active permutation is frozen for the turn rather than
      regenerated per frame, and `LayoutKind::resolve` supports ticket 03 drawing a
      fresh one per selection.
- [x] Every layout is a permutation: no duplicated band, no dropped present key,
      no band on a hole, no band on a thumb or underglow LED. Tests cover 26 keys
      left, 27 right, and the mirrored counterpart of every left key except the
      one documented in ticket 04's notes.
- [x] `palette::Palette` names all eight ramps and holds sorted `(position, Rgb)`
      stops. `palette::sample(palette, level, floor)` returns black below `floor`
      and otherwise interpolates the stops.
- [x] `ember` reproduces the current `render::ramp_with_floor` output exactly
      (regression test over sampled levels, including the floor boundary).
- [x] `render::compose` takes the layout and palette (a small `Look`/selection
      value is fine) instead of hard-coding `band_leds` and `ramp_with_floor`. The
      default selection produces the same frame as v0.1.
- [x] Thumb behaviour is unchanged: active palette for the level glow, white beat
      flash, no motion.
- [x] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

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

## Implementer handoff

Implemented on branch `look-variety`; ticket 02 is untouched.

- **Layout API (review fix).** Names live in `layout::LayoutKind`, which is
  `Copy` and has `ALL`, `NAMES`, `name`, `from_name`, `Display` and `FromStr`.
  `LayoutKind::resolve(&mut fastrand::Rng)` is the only way to obtain a concrete
  `layout::Layout`. Resolving `Scatter` always calls `Scatter::draw`; there is no
  identity/row-major scatter to fall into by accident. `Layout` is a
  data-carrying enum because scatter holds its frozen permutation, and
  `band_leds_for` takes `&Layout` so both halves can render that frozen value
  without cloning it into every frame. Ticket 03 should resolve once per
  selection and cache the resulting band table.
- **Snake equivalence.** `band_leds` is now a wrapper over
  `band_leds_for(&Layout::Snake, half)`. Its byte-identical output is pinned by a
  hard-coded test on both halves, separate from the geometric permutation tests.
- **Scatter mirroring.** The Fisher–Yates shuffle runs over the full 4×7
  normalised rectangle; each half then filters its own holes. Restriction of a
  uniform full-grid permutation is a uniform permutation of each half's present
  keys, and both halves see the same normalised order. The mirror test accounts
  for the board's single asymmetric pair: left `(3,0)` has no right counterpart,
  right `(1,0)`/`(2,0)` have no left counterparts.
- **Palette interpolation.** `palette::sample` takes segment spans in integer
  thousandths rather than subtracting the `f32` stop positions directly. This is
  what keeps ember byte-identical: `0.65f32 - 0.35f32` is one bit away from the
  v0.1 literal `0.30f32`, and a direct subtraction changes ember's colour at a
  few sampled levels. The regression test compares ember against a verbatim copy
  of the v0.1 ramp over 1001 levels plus several floors.
- **Composition.** `render::Look { layout, palette }` defaults to
  `snake`/`ember`; `compose(dynamics, count, &look)` uses it for both matrix
  bands and thumb level glow. Thumb beat flash remains hard white, independent of
  layout and palette.
- **Skipped:** static layout probes. They would add CLI scope, which the ticket
  made optional; ticket 04 can add a probe when it owns the `--probe-*` surface.

Validation run locally: `cargo test` (199 tests), `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`.
