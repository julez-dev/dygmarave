# 02: Capture the sink's monitor source

**What to build:** A probe that captures the default sink's monitor and reports live level, so you can confirm the daemon hears what your speakers play rather than what your microphone hears. The failure this guards against is silent and convincing: the microphone path produces a perfectly valid stream that shows the wrong thing, so a flat or implausible signal must be reported as an error naming the source, not displayed as success.

**Blocked by:** 01 (work proceeds in numeric order)

**Status:** ready-for-agent

- [ ] Capture follows the default output sink's monitor, not the default input device.
- [ ] The probe prints live level while music plays and reads silence when playback pauses.
- [ ] A stream that carries no signal, or that clearly comes from an input device, fails with a message naming the source used.
- [ ] Sample rate and channel count are reported so mismatches are visible.
