# 07: Sustained frame rate — fire-and-forget writes and adaptive pacing

**What to build:** Making the rate robust. Frames are written without waiting for each terminator, a periodic health probe notices a port that has stopped accepting writes, and the pacing loop lowers the frame rate when the wire cannot keep up rather than dropping bands or letting a backlog build. The daemon reports the rate it is actually achieving, so a slow board is visible rather than mysterious.

**Blocked by:** 05

**Status:** ready-for-agent

- [ ] Frames are written fire-and-forget, with no per-frame wait for the response terminator.
- [ ] Acknowledgements are drained unconditionally in the render loop, at a cadence that keeps the port from stalling over an hour-long show (ticket 01 drains every 64 frames as a stopgap).
- [ ] A port that stops accepting writes is detected by the health probe rather than silently dropping frames forever.
- [ ] When the board cannot sustain the target rate, fps degrades and recovers, and spectral fidelity is never reduced.
- [ ] The achieved rate is reported so it can be compared against the target.

## Not a reason to raise the frame rate

A layer change repaints the whole board from that layer's colormap, which wipes
whatever frame was showing. There is no host-side prevention: the Focus API has
no command to disable the colormap, lock the layer, or freeze the LED buffer, and
`led.mode` only chooses which effect owns the LEDs (0 is the colormap, 1–3
repaint continuously, 4+ is rejected by the firmware).

The maintainer chose to **accept the one-frame blink** rather than raise the
frame rate to shorten it (30 fps → ~33 ms; the measured ceiling of ~70 fps would
give ~14 ms but cost typing latency and CPU). So this ticket's job is the target
rate and the adaptive fallback, not chasing the flash.
