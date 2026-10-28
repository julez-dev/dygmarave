# 03: Look selection engine — pools, bags, timers, crossfade

**What to build:** The engine that turns three configured axes into a changing
selection. Each axis is fixed or random; random uses a shuffle bag over a pool;
each axis has its own jittered timer; any change crossfades from the colours
currently on screen. The engine is seeded and deterministic under `--seed`, and
it owns the fade state so `render::compose` stays pure.

**Blocked by:** 01, 02

**Status:** ready-for-agent

## Acceptance criteria

- [x] `look::AxisSettings<T>` (or equivalent) carries a fixed value or a random
      pool plus an interval. Fixed axes never draw from their pool/bag and never
      schedule. A fixed `LayoutKind::Scatter` still resolves one permutation from
      the run RNG at startup; that is resolution, not a pool draw.
- [x] A random axis draws at startup, before the first frame; the first switch
      happens one full interval later. Interval 0 draws once and never switches.
- [x] Each axis has an independent deadline. Jitter is uniform in ±25% of the
      interval and is not configurable.
- [x] `Bag<T>` draws without replacement, reshuffles when exhausted, and never
      repeats the last draw across the reshuffle boundary. A one-element pool
      repeatedly draws that element without reshuffle churn.
- [x] `scatter` is regenerated from the run RNG each time the layout bag draws it,
      then frozen for that turn.
- [x] The same `--seed` produces the same sequence of draws and scatter
      permutations; different seeds differ.
- [x] `LookState` holds the current selection, the target selection, the last
      displayed colours, and fade progress. On a switch it starts a fade from the
      last displayed colours. On a switch mid-fade it snapshots the current blend
      as the new start and restarts the fade to the new target. No discontinuity
      at either point.
- [x] `fade_ms = 0` cuts immediately. Fade alpha is wall-clock based, monotonic,
      and reaches the target exactly at the deadline. A smoothstep curve is fine;
      linear is acceptable if documented.
- [x] On selecting a different motion, call `MotionState::set_settings_at`/
      `set_motion_at` (or `reset_events_at`) with the current `Dynamics` **before**
      this frame's `motion.advance`. That clears pending `sparkle`/`ripple`/`rain`
      events, snapshots the beat counters, and leaves periodic phase continuous.
- [x] `MotionState` is the active motion selection plus runtime: the engine keeps
      the current `Motion`/`MotionSettings` for scheduling and logging, passes the
      state explicitly to `render::compose` alongside `Look`, and advances it once
      per frame.
- [x] The engine exposes the switch details needed by ticket 04 for logging
      (axis, from, to, reason).
- [x] Tests: bag fairness/no boundary repeat, one-element pool, jitter bounds,
      startup draw, interval 0, seed reproducibility, mid-fade retarget has no
      jump, fade completion, motion event reset.
- [x] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

## Notes

- Add `fastrand` and use `Rng::with_seed`; no other RNG source.
- The engine should take an explicit `now: Instant` so tests do not sleep. The
  live loop already passes `now` into `Live::step`.
- Store a `LayoutKind` in the settings and call `LayoutKind::resolve(&mut rng)`
  once when a selection is drawn. Cache the resolved band table for the turn;
  never redraw scatter or recompute the permutation per frame.
- On a live motion switch, use `MotionState::set_motion_at`/`set_settings_at` with
  the current `Dynamics` before the frame's `motion.advance`. Do not call the
  plain startup setters; make them test-only or remove them as part of this
  ticket so the swallowed-beat trap cannot return.
- `render::compose(dynamics, count, look, &motion)` already takes `MotionState`
  explicitly. Reuse one state per engine, not a new state per frame; the engine
  owns motion scheduling/logging and calls `advance` exactly once per frame.
- Switch ordering matters: call `reset_events_at(&dynamics)` (or the `_at`
  settings/motion variant) when the switch is applied, then call
  `motion.advance(now, &dynamics)` once for the frame. Do not call plain
  `reset_events()` on a live switch; it is the construction/startup path and
  suppresses a beat until the next advance.
- Suggested shape: `LookState::new(settings, seed, now) -> LookState`,
  `LookState::tick(now) -> Option<Switch>`, `LookState::selection() -> Selection`,
  `LookState::fade_alpha(now) -> f32`,
  `LookState::accept_frame(displayed: Vec<Rgb>, target: Vec<Rgb>)` or a small
  equivalent. Keep the colour blend in one function.
- Do not wire TOML/CLI yet; ticket 04 owns the user-facing surface. Use a
  settings struct with `Default` matching the v0.1 look.
- The live loop must remain allocation-free enough for 30 fps. Blending 132 LEDs
  is trivial; a per-frame `Vec` allocation is acceptable only if the existing
  compose path already does one.

## Implementer handoff

Implemented on branch `look-variety`; ticket 04 is untouched.

- **Shape.** `look::LookState` owns the run RNG, three `AxisRunner`s, the current
  `ResolvedLook`, one `MotionState`, and the fade snapshot. `Selection` is the
  three names; `ResolvedLook` is that selection plus the resolved `Layout` and a
  cached `(Cell, led)` table per half. `render::compose_resolved` uses those cached
  slices, so fade frames do not rerun a layout permutation. The old
  `render::compose` remains as a test-only simple entry point.
- **Tick signature.** `tick(now, &Dynamics) -> Vec<Switch>` rather than
  `tick(now)` because a live motion switch must call `set_settings_at` with the
  current `Dynamics` before the frame's `motion.advance`. It may return several
  switches when deadlines coincide. Startup draws emit no `Switch`; interval
  switches carry `reason = SwitchReason::Random`.
- **Startup and scatter.** Random axes draw once in `LookState::new` and schedule
  one jittered interval from `now`. Interval zero draws once and never schedules.
  A fixed `LayoutKind::Scatter` resolves once at startup. Every draw of
  `LayoutKind::Scatter` resolves a fresh Fisher–Yates permutation; a fresh
  scatter with the same kind is treated as a real change (from/to both
  `"scatter"`) and fades.
- **No-op draws.** A draw equal to the current value emits no switch and starts no
  fade, but the bag draw and deadline reschedule still happen. One-element bags
  return their item without consuming RNG; the timer still reschedules.
- **Crossfade.** `LookState` keeps `last_displayed`, a `fade_from` snapshot and
  `fade_started`. A switch snapshots the last displayed frame, applies the new
  selection, resets motion via `set_settings_at`, and starts a smoothstep fade.
  `fade_ms = 0` cuts. A mid-fade switch snapshots the current blend. At alpha 1
  the snapshot is cleared. `render` allocates the composed target and reuses the
  previous frame buffer for the returned/blended frame, so a fade frame keeps the
  existing one-Vec-per-frame budget.
- **Live.** `Live` owns `LookState::new(LookSettings::default(), 0, start)` for now
  (ticket 04 supplies the real seed/settings), calls `tick` once and then
  `render` once per frame. Plain `MotionState::set_settings`/`set_motion` are now
  `#[cfg(test)]`; production switches can only use the `_at` variants.
- **Not done here:** config, CLI, seed logging, switch logging and hardware pass
  remain ticket 04.

Validation run locally: `cargo test` (246 tests), `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`.

## Review fixes (ticket 03)

- **First bag draw.** `Bag` now shuffles its full remaining set before the first
  pop, so the first draw consumes the run RNG and depends on the seed. Pinned
  first-draw and seed-dependence tests cover it; the first cycle is still a
  permutation. `different_seeds_differ` now uses interval-zero random axes and
  compares `selection()`/`resolved()`.
- **Two RNG streams.** The selection stream uses the look seed for bags, scatter
  and jitter. The motion stream uses `seed ^ MOTION_SEED_SALT` for sparkle/rain
  randomness, so selection draws cannot perturb the motion show. Both are still
  reproducible from one seed.
- **Startup-only reset.** `MotionState::reset_events` is private again; live
  switches only have `reset_events_at`/`set_motion_at`/`set_settings_at`.
- **Acyclic modules.** `Dynamics`, `DynamicsConfig`, `BASS_BANDS` and the
  render floor moved to `src/dynamics.rs`. `render::compose_bands` is the single
  render primitive and takes resolved band tables; `compose_resolved` and the
  render → look import are gone. Production dependency edges are now
  `motion -> dynamics`, `render -> dynamics, motion`, and
  `look -> render, motion, dynamics`.
- **Hardening.** The same-tick test asserts the engine actually took all three
  selected values into `selection()`/`resolved()`. A late-poll test pins one
  switch, no repeat on the same deadline, and a reschedule of
  `[0.75, 1.25] × interval` from `now`.

Validation run locally: `cargo test` (248 tests), `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`.
