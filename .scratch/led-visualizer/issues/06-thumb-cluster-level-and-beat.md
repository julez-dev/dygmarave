# 06: Thumb cluster — per-channel level and beat flash

**What to build:** The thumb clusters join in: each half's cluster glows with its own channel's level, and a bass onset flashes it white before it settles back to the level colour. The matrix bands are untouched by this, so the spectrum stays readable while the thumbs carry a coarse loudness reading and the rhythm.

**Blocked by:** 05

**Status:** ready-for-agent

- [ ] Each half's thumb cluster reflects its own channel's level, not a shared value.
- [ ] A bass onset produces a brief white flash that returns to the level colour.
- [ ] Onsets do not double-flash on a single hit, and busy material does not leave the flash permanently on.
- [ ] Matrix band colours are unaffected by level or beat activity.
