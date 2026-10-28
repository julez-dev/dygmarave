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
      pool plus an interval. Fixed axes never draw and never schedule.
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
- [ ] On selecting a different motion, pending `sparkle`/`ripple`/`rain` events
      are cleared. Periodic motion phase is continuous.
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
