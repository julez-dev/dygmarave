# 03: Spectrum — 28 log-spaced bands per channel

**What to build:** Turning captured audio into a spectrum: a 1024-sample window per channel, folded into 28 log-spaced bands spanning 40 Hz to 16 kHz, printed as a readable live readout. The output is a plain frame of band energies per channel that knows nothing about keyboards, so the renderer can be built against it without the device in the room.

**Blocked by:** 02

**Status:** done

- [x] Each channel is analysed independently; a tone panned to one side shows energy on that side only.
- [x] A frequency sweep moves energy monotonically from the low bands to the high bands in the readout.
- [x] Band boundaries are log-spaced across 40 Hz–16 kHz, so bass and treble each own comparable bands.
- [x] The result is exposed as a frame of per-channel band energies with no device or rendering knowledge attached.

## Comments

Run with `cargo run -- --probe-spectrum [--seconds N] [--source ID]`. One character per band, 8 steps from silence to full scale, one group per channel.

**Evidence, measured through the monitor of a real sink**

| Check | Result |
| --- | --- |
| 1 kHz panned hard left | left channel peaks at band 15 (~1109 Hz); right channel entirely empty |
| 12 s log sweep, 60 Hz → 15 kHz | peak band strictly monotonic across the run; first run **6 → … → 27**, after the review fixes **4 → … → 26** (different start offsets, see below) |
| 60 Hz tone | peak band **3 (~85 Hz)**, energy 0.195 — the bass limitation, quantified |
| Energy accuracy | a 0.5 sine measures 0.47–0.50 depending on where it falls between bins |
| Log spacing | consecutive edge ratios equal within 1e-3; top band is >100× the width of the bottom band |

Two caveats on the live numbers, both recorded rather than smoothed over:

- **The live sweep never starts at band 0.** Capture takes a moment to open, so the trace begins 1–3 s into the sweep: the first run started at band 6 (~150 Hz), the second at band 4 (~100 Hz). Bands 0–2 were therefore never exercised by the live readout — the 60 Hz tone covers them instead, and the unit test covers the full 60 Hz → 15 kHz sweep without capture timing involved.
- **A 60 Hz tone reads as band 3 (~85 Hz).** That is the bass coarseness below, measured rather than estimated: a quarter-bin error at the very bottom of the range.

As an independent check on absolute frequency, after 11 s of the 12 s sweep the loudest band read ~9425 Hz; the sweep's own formula gives 9420 Hz at that moment.

**Two things that could have been wrong and are not**

- **Summing power across a tone's main lobe over-reads by √1.5**, the Hann window's equivalent noise bandwidth. Without the correction a 0.5 sine measures 0.61. The constant is named and derived in the source rather than tuned by eye.
- **Bands are integrated by overlapping power, not by picking the nearest bin.** Bins are 46.9 Hz wide — wider than the lowest bands (40–62 Hz) — so a nearest-bin rule would leave most bass bands empty. Each bin's power is split across the bands its frequency range overlaps.

**Known limitation, deliberately accepted:** below ~200 Hz the bands are narrower than the analysis window can resolve, so a low tone reads a band or three above its true frequency — a 60 Hz tone reads as 85 Hz. There is a test pinning this (`bass_is_coarser_than_one_band_per_hz_and_that_is_expected`). Fixing it needs a longer window, which would smear transients, or a lower sample rate, which would lose the treble. For a display where bass reads as a blur this is the right trade, but no future ticket should expect bass bands to be individually accurate — and ticket 06's beat detection should sum the lowest few bands rather than trust band 0.

**Interface for the renderer (ticket 04/05):** `Analyzer::push_interleaved` + `Analyzer::spectrum() -> Option<&Spectrum>`; `Spectrum::bands(channel)` gives linear **energies** where 1.0 is a band at full scale, in ascending frequency, so index 0 is bass — exactly the order ADR-0003 expects at the inner edge. The returned buffer is reused, so the renderer borrows it per frame rather than cloning.

## Review round

Two-axis review of `a91d155`. Both axes found the same P1, and the spec axis caught an overstatement in this ticket's own evidence.

- **P1 — a misaligned block silently swapped channels.** `push_interleaved` used `chunks_exact` and dropped the remainder. That is right for the stateless `level::measure`, but wrong for a stateful accumulator: one block ending mid-frame shifts every later sample by one, so left and right trade places for the rest of the stream. Samples now go into a `carry` and are prepended to the next block, with a regression test that pushes 101-sample blocks.
- **Band count claimed keyboard knowledge.** `BAND_COUNT` was documented as "matches the 28 matrix keys of one Dygma Raise half", contradicting the module's own promise that it knows nothing about keyboards. Reworded: 28 because a half has 28 keys, but the module only produces bands.
- **`Spectrum` was called "one frame of band amplitudes"**, and band **Energy** was called amplitude throughout — both on the glossary's `Avoid` lists (Frame means a whole keyboard colouring; Amplitude should be Energy). Renamed.
- **No Nyquist guard.** A Bluetooth call profile runs at 8 kHz, where 16 kHz exceeds Nyquist and the top six bands would be permanently dead *and* invisible, because the readout trims trailing blanks. The top edge is now pulled to 98% of Nyquist, keeping 28 live bands over a narrower range, with a test at 8 kHz.
- **Per-frame allocation** (`vec![0.0; bins]` and a fresh `Spectrum` every call) replaced with reused fields, so the renderer can call `spectrum()` every frame for free.
- **`fold` scanned all 513 bins** when at most ~342 can fall in a band; it now stops at the top edge.
- **This ticket's own evidence was wrong in one detail**: it implied the sweep trace started at band 6 because that is where 60 Hz lives. It does not — band 6 is ~150 Hz. The trace started there because capture opened 2.4 s into the sweep. Corrected above, and the 60 Hz tone was added to cover the bands the live run missed.
