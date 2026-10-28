# 04: Band sequence geometry — snake with mirrored halves

**What to build:** The index→key table that puts each band on the right physical key, and a static gradient drawn through it to prove the geometry by eye. Bands snake down a column and back up the next, the right half mirrors the left with the lowest band at the inner edge, and the underglow carries no band. The table comes from the board's own enumeration order, correlated against Dygma's published colour-map image — it is the one place a mistake shows up as a rotated or mirrored picture rather than an error.

**Blocked by:** 01

**Status:** done

- [x] A static gradient renders with bass at the board's centre and treble at the outer edges, looking symmetric across the two halves.
- [x] Frequency neighbours sit on physically adjacent keys within each half, with no discontinuity at row boundaries.
- [x] Underglow and neuron LEDs remain dark.
- [x] The table is derived from the observed enumeration order and is covered by a test against a recorded expectation.

## Resolving the two disagreeing Dygma sources

**Resolved with a photograph of the board.** Dygma's own two sources disagreed
about which LED sits on which key; the photo settled it in favour of Bazecor.

| LED | Dygma firmware (`key_led_map[4][16]`) | Bazecor (`Keymap-ANSI.jsx` `led_map[5][16]`) | Observed on a wired ANSI Raise |
| --- | --- | --- | --- |
| 0 | matrix (row 3, col 0) | physical (row 0, col 0) | **Escape, top-left** |
| 26 | matrix (row 0, col 6) | physical (row 4, col 0) | **left Ctrl, bottom-left** |

The firmware's map is 4 rows × 16 columns and is a *keymap* coordinate system;
Bazecor's is 5 rows × 16 columns of *physical* positions and is what the GUI
uses to colour keys. Both are shipped by Dygma. The observations match Bazecor
exactly and contradict the firmware map, so Bazecor's map is the one to trust.

**This also breaks a settled assumption.** The spec says 28 alphanumeric keys
per half plus 4 thumb keys, i.e. 64 key LEDs. Bazecor's map has **69 key LEDs**
(0–68) and underglow from 69. Counting Bazecor's grid per half:

| | rows 0–3 (letter rows) | bottom row | total |
| --- | --- | --- | --- |
| left | 7 + 6 + 6 + 7 = 26 | 7 | 33 |
| right | 7 + 7 + 7 + 6 = 27 | 8 | 35 |

So a half does **not** have 28 letter-row keys: the left has 26 and the right
27, because the physical grid has holes where Bazecor stores `XX`. Fitting 28
bands onto that geometry is a real decision, not an implementation detail — the
options are in the question I asked the maintainer, because each changes what
the user sees.

Also measured, and relevant to tickets 05 and 07: **a layer change repaints the
whole board**, wiping any written frame. Whether an ordinary letter key does the
same is unresolved (the earlier "colours reset" happened while the maintainer
was using a layer-modifier key).

## Outcome

The table lives in `src/layout.rs` as Bazecor's physical grid, corrected where the
photo contradicted it, and the shape is verified two ways: a column pattern whose
colours matched the photographed keys exactly, and a gradient the maintainer
confirmed — *"red at 6/7, blue at edges"*.

**What the photograph established**

- Column colours matched the grid left to right, and mirrored columns shared a
  colour: columns 0↔15, 1↔14 … all correct.
- Letter-row key counts: left 7,6,6,7 = **26**; right 7,7,7,6 = **27**. Neither
  half has the 28 the ADR assumed.
- The `\` key was dark, which is LED 20 — an index one measured unit refused to light.
  The map predicts it; the photo shows it. That is the strongest single piece of
  evidence for the table.
- LED 40 sits on no key, and Bazecor's raw map lists LED 29 twice; Bazecor's own
  device definition (left 0–32, right 33–68) resolves both.

**Deviation from ADR-0003, recorded there:** 28 bands are still analysed, but a
half shows 26 or 27 of them — the top band(s) have no key. The snake, mirroring
and bass-at-inner-edge all hold.

**Methods that failed, kept for the next person:** the firmware's
`key_led_map[4][16]` is a keymap map and produced a mirrored picture; the first
gradient went red-at-bottom-left because of it. Asking the maintainer to describe
positions verbally produced two data points that contradicted the firmware map —
which is what prompted the photo, and the photo was worth more than either.

## Review round

Two-axis review of `7a25162`. Both axes independently judged it a block, and both
were right about the substance rather than the tests:

- **P1 — the snake jumped three rows.** In a column-wise walk the left inner
  column has keys only at rows 0 and 3, so band 0 → band 1 jumped from LED 6 to
  LED 25. The test that was supposed to catch this
  (`bands_are_adjacent_or_one_column_apart`) only asserted that no key was reused,
  so it passed regardless — a test whose name promised adjacency it never checked.
  Fixed twice over: the walk now runs along **rows** (which are unbroken), and a
  real adjacency test measures the row/column distance between consecutive bands.
- **P1 — symmetry was claimed, not achieved.** I had written "the mirroring
  stands". It does not: rows hold 7,6,6,7 keys on the left and 7,7,7,6 on the
  right, so the halves cannot be exact mirror images. The ADR and this ticket now
  say so, a test pins the single left key with no counterpart, and the picture is
  mirrored everywhere it physically can be.
- **P1 — the table had no per-cell lock.** Set-level invariants (68 LEDs, unique,
  no LED 40) would have let a transposed pair through. The whole table is now
  asserted against a recorded literal.
- **P2** — `column_pattern` and `row_pattern` duplicated a loop; magic hue values
  undocumented; `--gradient` silently won over `--rows`/`--columns`.
