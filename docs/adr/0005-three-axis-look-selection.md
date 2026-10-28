# Three independent look axes, with shuffled bags and an engine-owned crossfade

The v0.1 board has one look: the snake band sequence, the ember ramp, and no
motion. The feature turns that into three independent axes — **layout** (which
key carries each band), **palette** (which colour a level becomes), and
**motion** (an additive modulation over the matrix). Each axis can be fixed or
random, and any change crossfades from the colours actually on screen.

## Decision

- **Three axes, one selection.** A `Selection` is a layout kind, a palette and a
  motion. Layout is resolved into a frozen permutation and a cached `(cell, LED)`
  table once per selection; palette and motion are simple values. The render path
  reads the cached table, not the permutation.
- **Random means a shuffle bag.** A random axis draws without replacement from
  its pool and reshuffles only at exhaustion. The first item of the new bag is
  prevented from equalling the last item of the old one, so a boundary cannot
  repeat. A one-element pool returns its element without RNG churn. A draw equal
  to the current value is a no-op: it advances the timer but emits no switch and
  starts no fade.
- **One jittered timer per axis.** Intervals are config-only. A random axis draws
  once at startup and schedules `now + interval` with uniform ±25% jitter, not
  configurable. Interval zero draws once and never schedules. A late tick emits
  one switch and reschedules from the current time, never a burst.
- **Seeded, with two streams.** The run seed drives the selection stream (bag
  draws, scatter permutations, timer jitter) and, with a constant salt, the
  motion event stream (sparkle keys, rain droplets). One seed reproduces both;
  selection draws cannot perturb the motion show.
- **Crossfade belongs to the look engine.** `render::compose_bands` renders one
  resolved selection and is pure over motion state. `LookState` owns the last
  displayed frame, the fade snapshot and the wall-clock deadline. On a switch it
  snapshots the displayed colours, applies the new selection, and starts a
  smoothstep fade from the snapshot. Only a *motion* switch resets motion events
  with the current `Dynamics`; layout and palette switches leave sparkle and
  ripple events running. A mid-fade switch snapshots the current blend and
  retargets. At alpha
  one the snapshot is cleared. `fade_ms = 0` cuts.
- **Motion events clear, phase continues.** A motion switch clears sparkle,
  ripple and rain state and snapshots the beat counters so a pre-switch beat
  cannot fire late. Layout and palette switches do not clear event state. The continuous motion clock is never reset by a switch or a
  fade.

## Considered Options

- **One combined "look" name** (rejected): layout, palette and motion vary along
  different schedules and should be selectable independently.
- **Uniform random choice per timer** (rejected): it repeats values by chance and
  feels like a bug. Shuffle bags guarantee coverage before repetition.
- **Fade inside `compose`** (rejected): `compose` would need mutable fade state and
  could not be called twice in one frame for a crossfade. The engine owns the
  blend and passes it a pure resolved selection.
- **Reset periodic phase on switch** (rejected): a fade or axis change would jump
  sweeps and orbits. Event state resets; phase does not.

## Consequences

- The default selection is still `snake` / `ember` / `none`, so an empty config
  renders the v0.1 frame.
- Layout changes rebuild one small cached band table; frame rendering does not
  recompute a permutation.
- Random rotation is reproducible with `--seed`; the effective seed and each
  switch are logged unless `--quiet`.
