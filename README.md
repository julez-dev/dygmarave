# dygmarave

Audio-reactive per-key LED visualisation for the Dygma Raise. dygmarave captures
the output mix (never the microphone), analyses it into 28 log-spaced bands, and
paints each half of the keyboard through a palette, a layout and an optional
motion.

## Build

```sh
cargo build --release
```

The board is found by USB identity. Bazecor must not be holding the serial port;
dygmarave pauses while Bazecor is running and reconnects when it exits.

## Configure

The config file lives at `$XDG_CONFIG_HOME/dygmarave/config.toml` (normally
`~/.config/dygmarave/config.toml`). A missing file means the defaults. Start from
`contrib/config.example.toml`, which documents every key.

The `[look]` table has three independent axes:

| Axis | Default | Values |
|---|---|---|
| `layout` | `snake` | `snake`, `vertical-snake`, `rows`, `columns`, `rings`, `diagonal`, `spiral`, `scatter` |
| `palette` | `ember` | `ember`, `ice`, `magma`, `neon`, `mono`, `spectrum`, `solar`, `forest` |
| `motion` | `none` | `none`, `sweep`, `sweep-vertical`, `sparkle`, `breathe`, `ripple`, `orbit`, `rain` |

Each axis can instead be `"random"`. A random axis draws from its pool without
replacement, reshuffles at exhaustion, never repeats across the boundary, and
switches on a jittered `*_interval_s` timer. Interval 0 draws once at startup and
never switches. An empty pool means every built-in value except `none` for
motion. `fade_ms` controls the crossfade; `motion_intensity` (0..2) and
`motion_speed` (0.25..4) tune motion.

An empty config still produces the original `snake`/`ember`/`none` look.

## Live

```sh
cargo run --release -- --live
```

The frame rate starts at 30 fps and lowers automatically when the board cannot
keep up. Use `--layout`, `--palette` and `--motion` to override a fixed axis or
request `random` (the pool and interval still come from `[look]`). `--seed`
makes random draws and motions reproducible; omit it for OS entropy. The
effective seed is printed once at startup.

```sh
cargo run --release -- --live --layout rings --palette ice --seed 42
```

`--quiet` suppresses the startup seed line and per-switch lines, never errors.
Switches are logged to stderr as `layout snake -> rings (random)`.

## Probes

The static hardware probes are not live layouts:

```sh
cargo run --release -- --probe-columns
cargo run --release -- --probe-rows
cargo run --release -- --probe-gradient
cargo run --release -- --probe-thumbs
```

`--probe-audio` prints a live level meter, `--probe-spectrum` prints the band
energies, and `--help` lists every flag. The old spellings `--columns`,
`--rows`, `--gradient`, `--thumbs` and `--thumb-pattern` remain hidden aliases.

## Test

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
