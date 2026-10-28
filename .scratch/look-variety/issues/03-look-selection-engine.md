# 03: Look selection engine — pools, bags, timers, crossfade

**What to build:** The engine that turns three configured axes into a changing
selection. Each axis is fixed or random; random uses a shuffle bag over a pool;
each axis has its own jittered timer; any change crossfades from the colours
currently on screen. The engine is seeded and deterministic under `--seed`, and
it owns the fade state so `render::compose` stays pure.

**Blocked by:** 01, 02

**Status:** ready-for-agent

## Acceptance criteria

- [ ] `look::AxisSettings<T>` (or equivalent) carries a fixed value or a random
      pool plus an interval. Fixed axes never draw from their pool/bag and never
      schedule. A fixed `LayoutKind::Scatter` still resolves one permutation from
      the run RNG at startup; that is resolution, not a pool draw.
- [ ] A random axis draws at startup, before the first frame; the first switch
      happens one full interval later. Interval 0 draws once and never switches.
- [ ] Each axis has an independent deadline. Jitter is uniform in ±25% of the
      interval and is not configurable.
- [ ] `Bag<T>` draws without replacement, reshuffles when exhausted, and never
      repeats the last draw across the reshuffle boundary. A one-element pool
      repeatedly draws that element without reshuffle churn.
- [ ] `scatter` is regenerated from the run RNG each time the layout bag draws it,
      then frozen for that turn.
- [ ] The same `--seed` produces the same sequence of draws and scatter
      permutations; different seeds differ.
- [ ] `LookState` holds the current selection, the target selection, the last
      displayed colours, and fade progress. On a switch it starts a fade from the
      last displayed colours. On a switch mid-fade it snapshots the current blend
      as the new start and restarts the fade to the new target. No discontinuity
      at either point.
- [ ] `fade_ms = 0` cuts immediately. Fade alpha is wall-clock based, monotonic,
      and reaches the target exactly at the deadline. A smoothstep curve is fine;
      linear is acceptable if documented.
- [ ] On selecting a different motion, call `MotionState::set_settings_at`/
      `set_motion_at` (or `reset_events_at`) with the current `Dynamics` **before**
      this frame's `motion.advance`. That clears pending `sparkle`/`ripple`/`rain`
      events, snapshots the beat counters, and leaves periodic phase continuous.
- [ ] `MotionState` is the active motion selection plus runtime: the engine keeps
      the current `Motion`/`MotionSettings` for scheduling and logging, passes the
      state explicitly to `render::compose` alongside `Look`, and advances it once
      per frame.
- [ ] The engine exposes the switch details needed by ticket 04 for logging
      (axis, from, to, reason).
- [ ] Tests: bag fairness/no boundary repeat, one-element pool, jitter bounds,
      startup draw, interval 0, seed reproducibility, mid-fade retarget has no
      jump, fade completion, motion event reset.
- [ ] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

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
