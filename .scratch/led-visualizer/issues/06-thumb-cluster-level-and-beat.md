# 06: Thumb cluster — per-channel level and beat flash

**What to build:** The thumb clusters join in: each half's cluster glows with its own channel's level, and a bass onset flashes it white before it settles back to the level colour. The matrix bands are untouched by this, so the spectrum stays readable while the thumbs carry a coarse loudness reading and the rhythm.

**Blocked by:** 05

**Status:** ready-for-agent

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

**Corrected after a first diagnostic run.** The first guess put the thumbs on
the underglow (69+). Running it showed all key LEDs dark while only underglow
lit — which is what the diagnostic did, but the wrong thing: the spec's "4 thumb
keys" means physical keys in the bottom row, not underglow. The constants are now
the inner-most bottom-row keys, two per side:

```rust
THUMB_CLUSTER_LEFT  = [31, 32]   // row 4, columns 6-7
THUMB_CLUSTER_RIGHT = [68, 67]   // row 4, columns 8-9, mirrored
```

These are key LEDs but *not* band LEDs: `band_leds` walks the four letter rows
only, so the thumbs and the spectrum never compete for the same LED.

`dygmarave --thumbs` paints these two clusters magenta (left) and cyan (right) and
leaves every other key and all underglow dark. A photograph while that runs shows
exactly which keys the guess lights. If the magenta/cyan keys are not the physical
thumb keys, correct the two constants — nothing else changes, because the
detection logic takes the indices as data.
