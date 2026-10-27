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

`dygmarave --thumbs` paints the guessed left cluster magenta and the right
cluster cyan against a dim grey backdrop of the rest of the underglow. A
photograph of the board while that runs shows both what was guessed and what was
left out. If the magenta/cyan LEDs are not the physical thumb keys, correct the
two constants — nothing else changes, because the detection logic takes the
indices as data.

The underglow is 63 LEDs (69–131), and the key map covers 0–68. The four-per-side
split chosen here leaves 55 LEDs for ambient underglow. A photograph settles the
real split the same way ticket 04's settled the key map.
