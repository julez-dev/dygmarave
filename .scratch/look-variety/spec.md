# Look variety — layouts, palettes, motions, random switching

Status: ready for implementation. Four tracer-bullet tickets in
`issues/`. Every decision below was settled in a grilling session; none is
provisional.

## Goal

The v0.1 renderer has exactly one look: the `snake` band sequence and the
`ember` ramp. This feature turns the look into three independent axes — **layout**,
**palette**, and **motion** — each selectable by name, and each optionally drawn
from a shuffle bag on its own jittered timer. Switches crossfade from whatever is
currently on screen.

The v0.1 look remains the default, so an existing config and an empty config
produce byte-identical frames.

Non-goals: position-aware palettes, motion as a replacement renderer, per-half
scanning, animated thumbs, beat-synced switching, configurable jitter, new
EEPROM-backed commands, Windows/macOS.

## Settled decisions

| Area | Decision |
|---|---|
| Axes | Layout (band→key permutation), palette (energy→colour ramp), motion (additive modulation). Independent. |
| Layouts | `snake`, `vertical-snake`, `rows`, `columns`, `rings`, `diagonal`, `spiral`, `scatter` — eight. |
| Palettes | `ember`, `ice`, `magma`, `neon`, `mono`, `spectrum`, `solar`, `forest` — eight. |
| Motions | `none`, `sweep`, `sweep-vertical`, `sparkle`, `breathe`, `ripple`, `orbit`, `rain` — seven plus none. |
| Invariant | Every layout is a permutation: exactly one distinct band per matrix key, never shared or dead. |
| Symmetry | One selection for the whole board; both halves mirror it. |
| Thumbs | Always level + white beat flash; level glow uses the active palette; motion never touches them; thumbs fade with the frame. |
| Selection | Fixed name or `"random"`. Random draws from a pool without replacement (shuffle bag); reshuffles at exhaustion, avoiding an immediate repeat across the boundary. |
| Pools | Read only when the axis is `"random"`. Empty/omitted = every built-in except `none`. `none` is drawable only if listed. Unknown names and duplicates are errors. |
| Timers | One per axis. `*_interval_s = 0` disables switching. ±25% jitter, not configurable. Draw at startup, first switch one interval later. A fixed `scatter` still resolves one permutation at startup; that is not a pool draw. |
| Transition | One `fade_ms` for all axes (default 300, 0 = hard cut). Fade starts from the colours actually on screen; a mid-fade switch snapshots the current blend and retargets. |
| Motion time | Periodic motions share one continuous clock; beat-driven events consume a monotonic beat counter, not the thumb flash; `sparkle`/`ripple`/`rain` clear pending events on switch. Fades never reset phase. |
| Seed | `--seed <u64>` CLI only; OS entropy otherwise. The effective seed is logged once at startup. `scatter` permutations derive from the same RNG. |
| Defaults | `layout="snake"`, `palette="ember"`, `motion="none"`, all intervals 0, `fade_ms=300`. |
| Config home | New `[look]` table. `[render]` stays signal dynamics. |
| CLI | `--layout`, `--palette`, `--motion` accept a name or `random`; `--seed`, `--quiet`. Pools stay config-only. |
| Probe flags | `--columns`, `--rows`, `--gradient`, `--thumbs` renamed `--probe-*` with old names as hidden aliases. |
| RNG | `fastrand` with `Rng::with_seed`. |

## Axis catalogues

### Layouts

Normalised half coordinates: `row` 0–3 (top to bottom), `depth` 0–6
(inner→outer). Holes are data: left lacks `(1,0)` and `(2,0)`; right lacks
`(3,0)`. Caps are 26 keys left and 27 right. (Both halves' holes are in the
inner column, which is why the v0.1 row snake was chosen over a column snake.)

| Name | Ordering |
|---|---|
| `snake` | Row by row, direction alternating: row 0 inner→outer, row 1 outer→inner, … This is the existing v0.1 order and must stay byte-identical. |
| `vertical-snake` | Column by column, direction alternating: inner column top→bottom, next bottom→top, … |
| `rows` | Row-major, every row inner→outer. Bass top-inner, treble bottom-outer. |
| `columns` | Column-major, every column top→bottom. Bass inner column, treble outer. |
| `rings` | Sort by squared distance from the inner home-row edge `d² = 4·depth² + (2·row − 3)²`, ties by row then depth. |
| `diagonal` | Sort by `row + depth`, ties by row. |
| `spiral` | Clockwise inward spiral over the 4×7 bounding rectangle starting at the inner-top cell; holes are skipped; unreached cells are appended row/depth order. Exact path written and tested in ticket 01. |
| `scatter` | Seeded Fisher–Yates shuffle of the full 4×7 normalised rectangle, regenerated on each draw of `scatter`, frozen for that turn and filtered to each half's present keys. |

All layouts are mirrored: the right half uses the same normalised ordering, mapped
to physical columns 9–15. `scatter` keeps that property by shuffling the full
rectangle before either half filters its holes. Both halves therefore consume the
same normalised order, and the restriction of a uniform full-rectangle permutation
to a half's present cells is a uniform permutation of those cells — so each half
is still fairly scattered despite the halves having different holes.

### Palettes

A palette is a list of `(position, Rgb)` stops interpolated piecewise; positions
ascending, first at 0.0, last at 1.0. The `floor` rule applies after sampling:
levels below `render.floor` are black. Suggested anchors (ticket 01 may tune them
for the board, but must preserve `ember` exactly):

| Name | Stops |
|---|---|
| `ember` | 0.00 black · 0.35 `(255,0,0)` · 0.65 `(255,128,0)` · 1.00 white |
| `ice` | 0.00 black · 0.35 `(0,32,120)` · 0.65 `(0,180,255)` · 1.00 white |
| `magma` | 0.00 black · 0.30 `(64,0,96)` · 0.60 `(200,0,64)` · 0.80 `(255,160,0)` · 1.00 `(255,255,220)` |
| `neon` | 0.00 black · 0.40 `(120,0,200)` · 0.70 `(255,0,128)` · 1.00 `(0,255,255)` |
| `mono` | 0.00 black · 0.35 `(64,64,64)` · 0.65 `(160,160,160)` · 1.00 white |
| `spectrum` | 0.00 black · 0.15 `(148,0,255)` · 0.30 `(0,64,255)` · 0.45 `(0,220,255)` · 0.60 `(0,255,96)` · 0.75 `(255,255,0)` · 0.90 `(255,64,0)` · 1.00 white |
| `solar` | 0.00 black · 0.35 `(80,40,0)` · 0.65 `(220,150,0)` · 0.85 `(255,230,120)` · 1.00 white |
| `forest` | 0.00 black · 0.35 `(0,64,24)` · 0.65 `(32,200,48)` · 0.85 `(160,255,96)` · 1.00 white |

`spectrum` is deliberately non-monotonic in luminance; every other palette should
read as a mostly monotonic brightening. The AGC ceiling is 0.7, so anchors are
chosen to look vivid by 0.7 and reserve pure white for 1.0.

### Motions

All motions are multiplicative/additive modulations of the composed matrix. The
spectrum must stay readable: no motion may blank a lit key for more than a frame
at a time, and `intensity = 0` is exactly identity.

| Name | Behaviour |
|---|---|
| `none` | Identity. |
| `sweep` | A smooth brightness crest travels inner→outer. `period = 3 s / speed`. Boost in `[1 − 0.6·intensity, 1]`; never below 0.3 brightness. |
| `sweep-vertical` | Same crest travelling top→bottom. |
| `sparkle` | On each channel's beat, light a few random keys on that half with a short exponential glint (~120 ms). Count scales with `intensity`. Event list cleared on switch. |
| `breathe` | Whole-matrix brightness follows the bass/thumb level: `1 − 0.35·intensity + 0.35·intensity·level`. |
| `ripple` | On a beat, an expanding ring starts at the inner edge. Ring radius grows to the half's extent in ~600 ms; brightness falls as it expands. Cleared on switch. |
| `orbit` | A soft bright spot orbits each half's matrix, mirrored, `period = 4 s / speed`. |
| `rain` | Deterministic droplets fall down random columns at `speed`; a key near a droplet brightens. Droplet state resets on switch. |

`motion_intensity` is validated 0..=2, `motion_speed` 0.25..=4.

## Composition

```
Dynamics (per channel bands, level, beat)
        ↓
palette(level, floor)          ← active palette
        ↓
layout places band n on key k  ← active layout permutation
        ↓
motion modulates matrix        ← active motion, clock, event state
        ↓
thumbs: palette level + white beat flash, motion excluded
        ↓
crossfade current screen → target selection, alpha over fade_ms
        ↓
Frame::from_colors
```

The crossfade blend state is owned by the look engine, not by `compose`:
`compose` renders one selection, and the engine blends the result with the last
displayed colours. This keeps `compose` pure and makes a mid-fade retarget
trivial: the last blended colours become the new start.

## Config

```toml
[look]
layout = "snake"           # or "random"
layouts = []               # pool, read only when layout = "random"
layout_interval_s = 0      # 0 = never switch; ±25% jitter
palette = "ember"          # or "random"
palettes = []
palette_interval_s = 0
motion = "none"            # or a motion name, or "random"
motions = []
motion_interval_s = 0
fade_ms = 300              # 0..2000, 0 = hard cut
motion_intensity = 1.0     # 0..2
motion_speed = 1.0         # 0.25..4
```

Validation is fail-fast and names the key, like the existing config:

- Names must be known; `"random"` is not a valid pool entry.
- Pools may not contain duplicates. `none` is valid only for the motion pool.
- Intervals must be finite and 0..=86400. `fade_ms` 0..=2000.
- `motion_intensity` 0..=2, `motion_speed` 0.25..=4.
- A random axis with interval 0 is legal: it draws once at startup and stays.

## CLI

- `--layout <name|random>`, `--palette <name|random>`, `--motion <name|random>`
- `--seed <u64>` — reproducible bags and scatter permutations.
- `--quiet` — suppress the startup seed line and per-switch lines; errors still print.
- Probes renamed: `--probe-columns`, `--probe-rows`, `--probe-gradient`,
  `--probe-thumbs`; the old names remain as hidden aliases.
- Pools are config-only. `--layout random` uses `[look] layouts`.

Startup logging: `seed=…` once, then one line per switch:
`layout snake -> rings (random)`.

## Module plan

- **`src/look.rs`** (new) — `Axis`, `Switch`, `AxisSettings`, `Bag`, the seeded
  run RNG, jittered per-axis deadlines, `ResolvedLook` with a cached band table,
  `LookState` and its crossfade.
- **`src/motion.rs`** (new) — `Motion` name layer and `MotionState`: the
  continuous clock, fixed sparkle/ripple/rain event buffers, `advance` for state,
  and read-only matrix modulation. `LookState` passes a `ResolvedLook` and the
  `MotionState` to `render::compose_resolved`.
- **`src/palette.rs`** (new) — named ramp stop tables and interpolation; `Rgb::lerp`.
- **`src/layout.rs`** — `LayoutKind` (names, `ALL`, `from_name`, `resolve`) plus the
  resolved `Layout` and `band_leds_for(&Layout, Half)`; keep the existing
  `band_leds` as the `snake` path or replace its call sites. Geometry tests for
  permutation and mirroring.
- **`src/dynamics.rs`** (new) — `Dynamics`, `DynamicsConfig`, `BASS_BANDS` and
  `FLOOR`, with the gating/AGC/beat-counter tests.
- **`src/render.rs`** — `compose_bands(dynamics, count, palette, left, right,
  motion)` paints using resolved band tables; test-only `compose` wraps it for
  the layout/palette API. `Pacer` unchanged.
- **`src/config.rs`** — `[look]` struct, defaults, validation, mapping to the
  engine's settings type.
- **`src/args.rs`** — new flags, probe aliases, help text.
- **`src/live.rs` / `src/main.rs`** — construct the look engine, pass the clock,
  apply the fade, log seed and switches.
- **`Cargo.toml`** — `fastrand`.

One test module per source file, per the repo convention.

## Testing

- Layout: each variant is a permutation of the present keys; capacities 26/27;
  mirrored; `snake` output equals v0.1; `scatter` deterministic from a seed.
- Palette: stop positions sorted; endpoints correct; `ember` equals the old ramp
  at 1000 sampled levels; floor applies; `spectrum` may be non-monotonic.
- Motion: `intensity = 0` identity; sweep/orbit periodicity; sparkle only on a
  beat counter edge (including 5 fps and overlapping beats); ripple radius grows;
  rain deterministic; event reset on switch; output clamped.
- Look: bag covers the pool before repeating; no repeat across a reshuffle;
  jitter inside ±25%; startup draw; fixed axes never draw; seed reproducibility;
  interval 0 never switches; mid-fade retarget has no discontinuity; `fade_ms = 0`
  cuts.
- Config: example config loads; defaults match v0.1; every invalid case names its
  key; unknown pool names and duplicates rejected.
- Args: names and `random` accepted; invalid names exit 2; probe aliases still work.
- Live: first frame with no config equals the v0.1 frame; switch lines appear;
  `--quiet` suppresses them.

## Docs

- New ADR-0005: three-axis look selection, shuffle bags, crossfade ownership.
- `GLOSSARY.md`: add **Layout**, **Palette**, **Motion**, **Switch**; amend
  **Band sequence** (a layout is one named band sequence) and **Ramp** (a palette
  is a named ramp).
- `contrib/config.example.toml`: add `[look]`, including a copy-paste
  "random everything" block.
- README / `--help` updated.

## Risks

- **Crossfade + low fps.** At the 5 fps floor a 300 ms fade is ~1.5 frames; the
  alpha is wall-clock based, so the look still lands, it just steps. No action.
- **Motion vs typing latency.** Every motion is a cheap per-LED arithmetic pass;
  none may allocate in the hot loop. A per-frame allocation is a defect.
- **Flicker.** `spectrum` plus `sparkle` at high intensity can be busy. Intensity
  is the knob; no strobe is in the catalogue.
- **Randomness felt as a bug.** Shuffle bags, no boundary repeats, and switch
  logging are the mitigation.
- **Layout geometry.** `rings` and `spiral` are the only layouts without an
  obvious photograph check; ticket 01 should provide a static probe for each and
  ticket 04 should photograph at least one.

## Tickets

1. `01-layout-permutations-and-palettes.md` — the eight layouts and eight ramps,
   wired into `compose` with fixed defaults.
2. `02-motion-modulations.md` — the seven motions plus `none`, with intensity and
   speed.
3. `03-look-selection-engine.md` — pools, shuffle bags, jittered timers, startup
   draw, crossfade from screen, scatter regeneration.
4. `04-config-cli-docs-hardware.md` — `[look]` config, CLI and seed/quiet, docs,
   and the hardware verification pass.
