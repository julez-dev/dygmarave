# 04: Band sequence geometry — snake with mirrored halves

**What to build:** The index→key table that puts each band on the right physical key, and a static gradient drawn through it to prove the geometry by eye. Bands snake down a column and back up the next, the right half mirrors the left with the lowest band at the inner edge, and the underglow carries no band. The table comes from the board's own enumeration order, correlated against Dygma's published colour-map image — it is the one place a mistake shows up as a rotated or mirrored picture rather than an error.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] A static gradient renders with bass at the board's centre and treble at the outer edges, looking symmetric across the two halves.
- [ ] Frequency neighbours sit on physically adjacent keys within each half, with no discontinuity at row boundaries.
- [ ] Underglow and neuron LEDs remain dark.
- [ ] The table is derived from the observed enumeration order and is covered by a test against a recorded expectation.
