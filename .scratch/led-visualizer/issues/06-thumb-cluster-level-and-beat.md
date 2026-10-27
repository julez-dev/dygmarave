# 06: Thumb cluster — per-channel level and beat flash

**What to build:** The thumb clusters join in: each half's cluster glows with its own channel's level, and a bass onset flashes it white before it settles back to the level colour. The matrix bands are untouched by this, so the spectrum stays readable while the thumbs carry a coarse loudness reading and the rhythm.

**Blocked by:** 05

**Status:** done

- [x] Each half's thumb cluster reflects its own channel's level, not a shared value.
- [x] A bass onset produces a brief white flash that returns to the level colour.
- [x] Onsets do not double-flash on a single hit, and busy material does not leave the flash permanently on.
- [x] Matrix band colours are unaffected by level or beat activity.

## Open verification — the thumb indices are a guess

**The logic is done and tested; the physical mapping is not verified.** Neither
Dygma source, the ADR, nor the spec records *which* underglow LEDs are the
physical thumb clusters. The constants in `src/layout.rs` are structural guesses:

```rust
THUMB_CLUSTER_LEFT  = [69, 70, 71, 72]
THUMB_CLUSTER_RIGHT = [128, 129, 130, 131]
```

**Verified on hardware.** The first guess put the thumbs on the underglow (69+).
Running it showed all key LEDs dark while only underglow lit — which is what the
diagnostic did, but the wrong thing: the spec's "4 thumb keys" means physical keys
in the bottom row, not underglow. Corrected to the inner-most bottom-row keys and
**confirmed by the maintainer on the board**: "thumb keys are glowing as
expected". The constants are:

```rust
THUMB_CLUSTER_LEFT  = [31, 32]   // row 4, columns 6-7
THUMB_CLUSTER_RIGHT = [68, 67]   // row 4, columns 8-9, mirrored
```

These are key LEDs but *not* band LEDs: `band_leds` walks the four letter rows
only, so the thumbs and the spectrum never compete for the same LED.

`dygmarave --thumbs` paints these two clusters magenta (left) and cyan (right) and
leaves every other key and all underglow dark. That is how the position was
confirmed; it stays as a diagnostic if the mapping is ever doubted.

## Review round

Code review returned **OK with notes** — all four criteria met, no P0/P1. The
behavioural finding was folded; the rest were doc and robustness cleanups.

- **P2 — beat detection bypassed the gate.** `bass_energy` summed the raw
  spectrum, ignoring the freshly-computed `open` flag. A sub-gate transient (hum,
  handling noise) would flash the thumbs white while the matrix stayed dark,
  reintroducing for the thumbs the "silence leaves the board dark" invariant the
  module exists to keep. Now a gated-closed channel contributes zero energy, and
  `silence_leaves_the_thumbs_dark` uses a realistic −70 dBFS floor rather than
  exactly zero, which would have passed either way and hidden the bug.
- **P2 — the `compose` doc comment still claimed the thumbs stay black.** Corrected.
- **P3 — the flash could vanish at low frame rates.** Setting the timer then
  decrementing it by `dt` erased the flash outright when one frame's `dt` exceeded
  `flash_ms` (`--fps` 1–10). The timer now decays *before* the trigger is set, so
  the trigger frame is always visible.
- **P3 — a possible `needless_range_loop`.** Rewritten as a slice fold.
