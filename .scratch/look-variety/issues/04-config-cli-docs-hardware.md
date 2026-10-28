# 04: `[look]` config, CLI, docs, hardware pass

**What to build:** The user-facing surface for the look engine, the documentation,
and a hardware verification run. An empty config still produces the v0.1 board;
`[look]` unlocks variety; the CLI can override the three axis values, seed the
draws, and quiet the switch log.

**Blocked by:** 03

**Status:** ready-for-agent

## Acceptance criteria

- [ ] `config::LookConfig` deserialises `[look]` with `layout`, `layouts`,
      `layout_interval_s`, `palette`, `palettes`, `palette_interval_s`, `motion`,
      `motions`, `motion_interval_s`, `fade_ms`, `motion_intensity`,
      `motion_speed`.
- [ ] Defaults are exact: `snake`, `ember`, `none`, intervals 0, `fade_ms = 300`,
      intensity 1.0, speed 1.0. A no-file run renders the same first frame as
      v0.1.
- [ ] Validation is fail-fast and names the key: unknown axis/pool names, duplicate
      pool entries, `none` outside the motion pool, intervals outside 0..=86400,
      fade outside 0..=2000, intensity outside 0..=2, speed outside 0.25..=4.
- [ ] `contrib/config.example.toml` documents every `[look]` key and includes a
      copy-paste "random everything" example with sensible intervals.
- [ ] `--layout`, `--palette`, `--motion` accept a name or `random`; invalid names
      exit 2. Pools remain config-only.
- [ ] `--seed <u64>` seeds the engine; without it, OS entropy is used. The
      effective seed is logged once at startup.
- [ ] One stderr line per switch, e.g. `layout snake -> rings (random)`;
      `--quiet` suppresses the seed line and switch lines, never errors.
- [ ] `--columns`, `--rows`, `--gradient`, `--thumbs` are renamed
      `--probe-columns`, `--probe-rows`, `--probe-gradient`, `--probe-thumbs`;
      the old spellings remain hidden aliases. Help text explains these are static
      hardware probes, not live layouts.
- [ ] `live.rs`/`main.rs` wire the engine: build settings from config, apply CLI
      overrides, seed, compute fade alpha, blend, log switches. The frame loop
      remains paced and the audio path unchanged.
- [ ] Docs: ADR-0005 (three-axis look selection, bag semantics, crossfade
      ownership), `GLOSSARY.md` entries for Layout/Palette/Motion/Switch with
      amended Band sequence and Ramp, README and `--help` updated.
- [ ] Hardware pass: run each of the eight layouts and eight palettes at least once
      (probe or live), each motion, and a 10-minute random rotation. Confirm no
      flicker beyond the accepted layer-change blink, no visible typing latency
      change, and that the thumbs keep working. Photograph `rings` and `spiral`.
      Record findings under `## Hardware` below.
- [ ] Full test suite, `cargo clippy -- -D warnings`, `cargo fmt --check` clean.

## Review round

(To be added after implementation.)

## Hardware

(To be filled in from the board.)

## Comments

(Add decisions or follow-ups raised during implementation.)
