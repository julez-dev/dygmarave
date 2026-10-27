# 01: Prove the LED path and measure the frame budget

**What to build:** A runnable command that finds the Raise without being told a port, takes it out of any repainting LED mode into a static one, paints every LED a single solid colour, and measures how many full `led.theme` writes per second the board actually sustains. On exit the LED mode and the baseline theme are both restored, so running it leaves the keyboard exactly as it was found. This ticket also brings the crate into existence.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] The crate builds and runs, and locates the device by Dygma USB identity rather than a hardcoded port name.
- [ ] The LED count is learned from the board's own reply, never from a constant, and printed so the 131-vs-132 question is settled on the real hardware.
- [ ] The board holds a solid colour for several seconds without an active effect overwriting it.
- [ ] A frames-per-second figure for full-frame writes is printed, measured rather than estimated.
- [ ] Exiting restores both the previous LED mode and the baseline theme.
