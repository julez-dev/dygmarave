# Bands snake down and back up columns

The matrix of each half is 7 columns × 4 rows, with 28 bands assigned one per key. The obvious mapping — reading order, low to high across each row — puts a hard frequency discontinuity at every row boundary, so physically adjacent keys show wildly different frequencies and the spectrum looks torn. Instead the band sequence runs **down one column and back up the next** (boustrophedon), keeping frequency neighbours physically adjacent so the render reads as a continuous gradient.

## Consequences

- The right half mirrors the left, with the lowest band at the inner (centre) edge, so bass sits at the middle of the board and treble fans outward.
- Band index → key position is not an arithmetic row-major formula; it lives in one explicit lookup table that must be tested against the firmware's LED index map for the ANSI Raise.
- Reverting to reading order is cheap, which is why this ADR exists mainly to stop someone "simplifying" the table into a row-major loop without knowing the reason.

## Measured correction (ticket 04)

The *shape* above survived contact with the hardware; the *arithmetic* did not.

A photographed board, with every column painted a different colour, shows the
physical grid is Bazecor's 5×16 map (`Keymap-ANSI.jsx`), not the firmware's 4×16
`key_led_map`: that one is a keymap coordinate system and puts LED 0 at a
different place than the board does. Verified anchors: LED 0 is Escape
(top-left), LED 26 is left Ctrl (bottom-left), and the two halves mirror about
columns 0↔15.

Consequences:

- **A half does not have 28 letter keys.** The four letter rows hold **26** keys
  on the left (7, 6, 6, 7) and **27** on the right (7, 7, 7, 6), because the
  physical grid has holes where a column ends early.
- **28 bands are still produced** by the analysis, but a half displays 26 or 27
  of them: the highest band(s) have no key on that half. Truncating the top of
  the spectrum is preferable to compressing 28 bands into 26 unevenly, and the
  lost bands are the least visible.
- **The board has 68 key LEDs, not 64.** LED 40 sits on no key, and LED 20 —
  the `\` key — never lights up at all on this unit, confirmed in the photo.
- The snake, the mirroring, and bass-at-the-inner-edge all stand as decided.
