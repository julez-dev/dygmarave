# 05: Live visualizer — spectrum to board

**What to build:** The first ticket where the product exists: music playing on the computer drives the board, continuously. Band energies pass through the colour ramp, instant attack with slow decay, per-band AGC and a silence gate before being written as full frames at a steady pace. Quiet passages go black rather than showing amplified noise, and quiet masters still light the board.

**Blocked by:** 03, 04

**Status:** ready-for-agent

- [ ] Playing music produces a moving spectrum on the board that tracks the audio, end to end.
- [ ] Transients pop immediately and fade smoothly afterwards, with no strobing at the frame rate.
- [ ] Silence, and audio below the gate, leaves the board dark instead of showing amplified noise.
- [ ] A quietly mastered track still fills the board, and a loud one does not clip into a wall of white.
- [ ] Frame pacing is steady with no unbounded backlog; latency does not drift away from the music over minutes.
