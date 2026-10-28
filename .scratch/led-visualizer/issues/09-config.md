# 09: Config — XDG TOML

**What to build:** Everything that turned out to be worth tuning becomes adjustable without a rebuild: device path, audio source, decay, gate, AGC release, ramp bounds, target frame rate and the thumb settings. Config lives at the XDG location with a documented example, command-line flags override it, and bad values are rejected at startup with a clear message rather than surfacing as odd lighting mid-song.

**Blocked by:** 05

**Status:** done

- [x] A config file at the XDG path is loaded, with a documented example covering every supported key.
- [x] Command-line flags override the file for the values they set.
- [x] Invalid or out-of-range values are rejected at startup, naming the offending key.
- [x] Changing decay, gate, AGC release, ramp bounds or the fps target takes effect without recompiling.

## Review round

Code review returned **OK with notes**. Folded:

- **P1 — `[analysis]` was dead config.** The window and band edges were validated and
  documented but ignored: `BandPlan::standard` hardcoded the constants. Both production
  sites now build the plan from config via a `band_plan` helper. A user setting
  `window = 2048` now takes effect instead of silently doing nothing.
- **P1 — example contradicted the spec.** `band_max_hz = 12000.0` in the example vs the
  spec's 16 kHz default. Set to 16000.0.
- **P2 — `render.floor` bounds.** Accepted 0.0 (disabling the silence floor, violating the
  "silence goes dark" invariant) and 1.0 (permanently dark). Now `0.0 < floor <= 0.5`.
- **P2 — duplicated defaults.** `RenderConfig::default()` and `ThumbConfig::default()`
  re-stated `DynamicsConfig`/`LiveConfig` numbers. They now delegate to those types.
- **P2 — compose branches untested.** Added tests for a non-default floor (fade through
  the floor), `thumb_glow = false`, and `beat_flash = false`.
- **P3 — silent fallback for an unreadable config.** A directory or permission-denied file
  was ignored; now only `NotFound` falls back, other errors propagate.
- **P3 — `HOME` not absolute-filtered.** Now filtered like `XDG_CONFIG_HOME`.
- **P3 — misleading test name.** Renamed to say what it actually asserts.

Skipped as judgement calls: the `#[cfg_attr(not(test), allow(dead_code))]` markers the user
requested for test-only constructors.
