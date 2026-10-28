# 04: `[look]` config, CLI, docs, hardware pass

**What to build:** The user-facing surface for the look engine, the documentation,
and a hardware verification run. An empty config still produces the v0.1 board;
`[look]` unlocks variety; the CLI can override the three axis values, seed the
draws, and quiet the switch log.

**Blocked by:** 03

**Status:** ready-for-agent

## Acceptance criteria

- [x] `config::LookConfig` deserialises `[look]` with `layout`, `layouts`,
      `layout_interval_s`, `palette`, `palettes`, `palette_interval_s`, `motion`,
      `motions`, `motion_interval_s`, `fade_ms`, `motion_intensity`,
      `motion_speed`.
- [x] Defaults are exact: `snake`, `ember`, `none`, intervals 0, `fade_ms = 300`,
      intensity 1.0, speed 1.0. A no-file run renders the same first frame as
      v0.1.
- [x] Validation is fail-fast and names the key: unknown axis/pool names, duplicate
      pool entries, `none` outside the motion pool, intervals outside 0..=86400,
      fade outside 0..=2000, intensity outside 0..=2, speed outside 0.25..=4.
- [x] `contrib/config.example.toml` documents every `[look]` key and includes a
      copy-paste "random everything" example with sensible intervals.
- [x] `--layout`, `--palette`, `--motion` accept a name or `random`; invalid names
      exit 2. Pools remain config-only.
- [x] `--seed <u64>` seeds the engine; without it, OS entropy is used. The
      effective seed is logged once at startup.
- [x] One stderr line per switch, e.g. `layout snake -> rings (random)`;
      `--quiet` suppresses the seed line and switch lines, never errors.
- [x] `--columns`, `--rows`, `--gradient`, `--thumbs` are renamed
      `--probe-columns`, `--probe-rows`, `--probe-gradient`, `--probe-thumbs`;
      the old spellings remain hidden aliases. Help text explains these are static
      hardware probes, not live layouts.
- [x] `live.rs`/`main.rs` wire the engine: build settings from config, apply CLI
      overrides, seed, compute fade alpha, blend, log switches. The frame loop
      remains paced and the audio path unchanged.
- [x] Docs: ADR-0005 (three-axis look selection, bag semantics, crossfade
      ownership), `GLOSSARY.md` entries for Layout/Palette/Motion/Switch with
      amended Band sequence and Ramp, README and `--help` updated.
- [ ] Hardware pass: run each of the eight layouts and eight palettes at least once
      (probe or live), each motion, and a 10-minute random rotation. Confirm no
      flicker beyond the accepted layer-change blink, no visible typing latency
      change, and that the thumbs keep working. Photograph `rings` and `spiral`.
      Record findings under `## Hardware` below.
- [x] Full test suite, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

## Review round

(To be added after implementation.)

## Hardware

**Pending controller/user verification. Nothing below has been run by the
implementer; no visual or hardware claim is made here.**

Run these only with the user present and audio playing where a lit board is
needed. `timeout -s INT` sends Ctrl-C so the restore path runs.

Build once first, then run the probes (photograph the result):

```sh
cargo build --release
cargo run --release -- --probe-columns
cargo run --release -- --probe-rows
cargo run --release -- --probe-gradient
cargo run --release -- --probe-thumbs
```

Each layout, then each palette:

```sh
for layout in snake vertical-snake rows columns rings diagonal spiral scatter; do
  timeout -s INT 20s cargo run --release -- --live --layout "$layout" --palette ember --motion none --seed 1
done
for palette in ember ice magma neon mono spectrum solar forest; do
  timeout -s INT 20s cargo run --release -- --live --layout snake --palette "$palette" --motion none --seed 1
done
```

Each motion:

```sh
for motion in none sweep sweep-vertical sparkle breathe ripple orbit rain; do
  timeout -s INT 20s cargo run --release -- --live --layout snake --palette ember --motion "$motion" --seed 1
done
```

Ten-minute random rotation:

```sh
mkdir -p /tmp/dygmarave-hardware/dygmarave
cat > /tmp/dygmarave-hardware/dygmarave/config.toml <<'CFG'
[look]
layout = "random"
layouts = ["snake", "vertical-snake", "rows", "columns", "rings", "diagonal", "spiral", "scatter"]
layout_interval_s = 20.0
palette = "random"
palettes = ["ember", "ice", "magma", "neon", "mono", "spectrum", "solar", "forest"]
palette_interval_s = 15.0
motion = "random"
motions = ["sweep", "sparkle", "breathe", "ripple", "orbit", "rain"]
motion_interval_s = 12.0
fade_ms = 300
motion_intensity = 1.0
motion_speed = 1.0
CFG
XDG_CONFIG_HOME=/tmp/dygmarave-hardware timeout -s INT 600s cargo run --release -- --live --seed 7
```

Photograph `rings` and `spiral` during their live runs. Observe flicker beyond the
accepted layer-change blink, typing latency, thumb level/flash behaviour, and
restoration after each Ctrl-C.

## Comments

(Add decisions or follow-ups raised during implementation.)

## Implementer handoff

Implemented on branch `look-variety`; no hardware commands were run.

- `[look]` config with strict validation and mapping to `look::LookSettings`.
  Pools are validated whenever present, even while the axis is fixed; they are
  only *read* when the axis is `"random"`. Empty pools use the built-ins (motion
  excludes `none`; an explicit `none` is allowed).
- CLI axes accept a fixed name or `random`, with invalid names exiting 2 through
  clap. Pools/intervals stay config-only. `--seed` falls back to OS entropy;
  `--quiet` suppresses the seed and switch lines.
- Probes are now `--probe-*`; old spellings are hidden aliases. Help calls them
  static hardware probes.
- `Live` owns `LookState`, logs each `Switch` to stderr, and `main` passes the
  seed, settings and quiet flag. Defaults still produce the v0.1 frame.
- Docs added: ADR-0005, README, glossary Layout/Palette/Motion/Switch plus
  amended Band sequence/Ramp. Hardware verification is pending above.

Validation run locally: `cargo test` (262 tests), `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`.

## Review hardening

- Seed and switch logs are testable seams: `main::seed_line(seed, quiet, live)`
  and `live::switch_lines(&switches, quiet)`. Tests cover zero/one/many switches,
  same-tick lines, quiet suppression, probe runs emitting no seed line, and the
  exact `layout snake -> rows (random)` format.
- `look.fade_ms` deserialises as an integer-or-float number, validates as a whole
  number in 0..=2000, and only then converts to `u32`. Negative and fractional
  values now fail naming `look.fade_ms`.
- `Live::with_look` takes a `LiveOptions` parameter object; the positional
  eight-argument constructor and its clippy allow are gone. `Live::new` remains
  the defaulting wrapper.
- `Config::look_settings` is `pub(crate)` and keeps the validated-input
  invariant. A fixed axis with an invalid unused pool still fails on
  `look.layouts`.
