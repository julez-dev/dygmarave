# Bands snake down and back up columns

The matrix of each half is 7 columns × 4 rows, with 28 bands assigned one per key. The obvious mapping — reading order, low to high across each row — puts a hard frequency discontinuity at every row boundary, so physically adjacent keys show wildly different frequencies and the spectrum looks torn. Instead the band sequence runs **down one column and back up the next** (boustrophedon), keeping frequency neighbours physically adjacent so the render reads as a continuous gradient.

## Consequences

- The right half mirrors the left, with the lowest band at the inner (centre) edge, so bass sits at the middle of the board and treble fans outward.
- Band index → key position is not an arithmetic row-major formula; it lives in one explicit lookup table that must be tested against the firmware's LED index map for the ANSI Raise.
- Reverting to reading order is cheap, which is why this ADR exists mainly to stop someone "simplifying" the table into a row-major loop without knowing the reason.
