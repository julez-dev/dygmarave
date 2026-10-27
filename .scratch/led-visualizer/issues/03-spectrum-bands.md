# 03: Spectrum — 28 log-spaced bands per channel

**What to build:** Turning captured audio into a spectrum: a 1024-sample window per channel, folded into 28 log-spaced bands spanning 40 Hz to 16 kHz, printed as a readable live readout. The output is a plain frame of band energies per channel that knows nothing about keyboards, so the renderer can be built against it without the device in the room.

**Blocked by:** 02

**Status:** ready-for-agent

- [ ] Each channel is analysed independently; a tone panned to one side shows energy on that side only.
- [ ] A frequency sweep moves energy monotonically from the low bands to the high bands in the readout.
- [ ] Band boundaries are log-spaced across 40 Hz–16 kHz, so bass and treble each own comparable bands.
- [ ] The result is exposed as a frame of per-channel band energies with no device or rendering knowledge attached.
