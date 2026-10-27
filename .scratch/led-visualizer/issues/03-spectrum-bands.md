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
| 12 s log sweep, 60 Hz → 15 kHz | peak band **6 → 8 → 10 → 12 → 14 → 16 → 19 → 21 → 23 → 25 → 27**, strictly monotonic, ending in the top band |
| Amplitude accuracy | a 0.5 sine measures 0.47–0.5 depending on where it falls between bins |
| Log spacing | consecutive edge ratios equal within 1e-3; top band is >100× the width of the bottom band |

The sweep summary is a second, independent check: after 11 s of a 12 s sweep the loudest band read ~9425 Hz, and the sweep's own formula gives 9420 Hz at that time.

**Two things that could have been wrong and are not**

- **Summing power across a tone's main lobe over-reads by √1.5**, the Hann window's equivalent noise bandwidth. Without the correction a 0.5 sine measures 0.61. The constant is named and derived in the source rather than tuned by eye.
- **Bands are integrated by overlapping power, not by picking the nearest bin.** Bins are 46.9 Hz wide — wider than the lowest bands (40–62 Hz) — so a nearest-bin rule would leave most bass bands empty. Each bin's power is split across the bands its frequency range overlaps.

**Known limitation, deliberately accepted:** below ~150 Hz the bands are narrower than the analysis window can resolve, so a low tone's energy legitimately straddles two or three neighbours (there is a test pinning this, `bass_is_coarser_than_one_band_per_hz_and_that_is_expected`). Fixing it needs a longer window, which would smear transients. For a display where bass reads as a blur this is the right trade, but no future ticket should expect bass bands to be individually accurate.

**Interface for the renderer (ticket 04/05):** `Analyzer::push_interleaved` + `Analyzer::spectrum() -> Option<Spectrum>`; `Spectrum::bands(channel)` gives linear amplitudes where 1.0 is a full-scale sine. No device, keyboard or terminal types appear in `spectrum.rs`.
