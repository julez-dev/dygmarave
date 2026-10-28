# 10: CLI parsing and application errors

**What to build:** The hand-rolled argument parser and the five hand-written error
types are replaced by the idiomatic crates: `clap` (derive) for the command line,
`thiserror` for the typed error enums, and `anyhow` at the application boundary.
The flag surface stays identical, but the help text, usage errors and exit codes
come from clap, so the parser and the help can no longer drift apart. The five
`Display` implementations become `#[error(...)]` attributes on the enums, and
`run()` returns `anyhow::Result` so the outermost message carries a context chain.

**Blocked by:** 09

**Status:** done

- [x] `clap` derive replaces the hand-rolled parser and the `USAGE` const; the flag set, aliases and the `--probe-spectrum` implication are unchanged.
- [x] `--live` conflicts with `--probe-audio` and `--probe-spectrum` declaratively, replacing the hand-wrapped conflict string that printed 21 literal spaces.
- [x] `thiserror` replaces the hand-written `Display` impls on `SessionError`, `ThemeError`, `AudioError`, `LinkError` and `ConfigError`.
- [x] `run()` and its helpers return `anyhow::Result`; `main` prints the context chain with `{:#}`.
- [x] Usage errors exit 2 and `--help`/`--version` exit 0, matching clap's conventions.
- [x] Every existing argument test still passes; new tests cover the declarative conflicts and the help/version exit paths.

## Decisions

- **`clap` v4 derive**, not the builder API. The flag set is small and the doc
  comments already describe each field, so the derive is a direct fit.
- **`thiserror` + `anyhow`, not one error crate.** `thiserror` keeps the five
  domain errors typed and matchable below the boundary; `anyhow` gives `run()`
  contextual chaining without erasing those types.
- **`--version` and `-V` are new.** clap adds both for free; the binary previously
  had no version flag. Recorded here so the frozen-surface claim is not read as
  "no new flags".
- **Value-parser messages lost their flag prefix.** `expects three comma-separated
  values …` instead of `--colour expects …`, because clap already prepends
  `invalid value … for '--colour <COLOUR>'`. Deliberate, not an oversight.

## Review round

Two-axis review (Standards and Spec, parallel). Both independently found the same
**P1**, which is fixed:

- **P1 — duplicate cause in the `{:#}` chain.** The old `impl std::error::Error`
  never exposed `source()`, so `main`'s `{:#}` printed each message once. The
  thiserror conversion accidentally introduced chaining: `ConfigError` had a field
  literally named `source`, and `SessionError` used `#[error(transparent)]` +
  `#[from]`. Both make thiserror wire `source()`, so `{:#}` appended the leaf a
  second time (`…: Permission denied: Permission denied`). Fixed by restoring the
  pre-diff semantics: `ConfigError`'s field is now `cause` (no auto-source) and
  `SessionError::Link/Theme` use `#[error("{0}")]` with manual `From` impls, so
  neither chains. Regression tests assert `source()` is `None` and the cause
  appears once. Verified at runtime: a malformed config prints the TOML block once.

Folded:

- **P2 — duplicated device-open block.** The three identical
  `from_port(..)/discover(..)` matches became one `open_device(&Config)` helper.
- **P2 — unused derives.** `Clone, PartialEq, Eq` on `Args` were left over from the
  deleted `Args::default()` comparison; the test now compares fields. Derive is
  `Debug, Parser`.
- **P2 — `parse_from` collided with clap's `Parser::parse_from`** (which exits
  instead of returning). Renamed to `parse_args`; the only `parse_from` left is the
  clap call `Args::try_parse_from`.
- **P2 — exit code 2 untested.** The conflict and unknown-flag tests now assert
  `error.exit_code() == 2`.
- **P2 — uneven context chain.** `Session::open`/`open_with` had no context while
  port discovery did; both now carry `opening a session with the board`.

173 tests, clippy (`-D warnings`) and `cargo fmt --check` clean.
