# 08: Lifecycle — reconnect, Bazecor pause, baseline restore, systemd unit

**What to build:** Surviving the real world of a desk. Unplugging the board mid-song does not kill the daemon: it keeps the audio pipeline alive, retries, and resumes with the correct LED count and mode when the board returns. Launching Bazecor pauses writes instead of corrupting the command stream, and resuming once it exits. A clean shutdown restores the baseline theme and the original LED mode, and a systemd user unit runs the whole thing at login without its restart policy fighting the daemon's own retry loop.

**Blocked by:** 01, 05

**Status:** ready-for-agent

- [ ] Unplugging and replugging mid-show resumes rendering without a restart, with the LED count and mode re-established.
- [ ] Bazecor running pauses writes; quitting it resumes them.
- [ ] SIGTERM and clean exit restore the baseline theme and the mode that was in use before startup.
- [ ] A systemd user unit starts the daemon at login, and systemd does not restart it in a loop while the daemon is retrying internally.
