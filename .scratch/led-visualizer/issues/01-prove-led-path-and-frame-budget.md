# 01: Prove the LED path and measure the frame budget

**What to build:** A runnable command that finds the Raise without being told a port, takes it out of any repainting LED mode into a static one, paints every LED a single solid colour, and measures how many full `led.theme` writes per second the board actually sustains. On exit the LED mode and the baseline theme are both restored, so running it leaves the keyboard exactly as it was found. This ticket also brings the crate into existence.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] The crate builds and runs, and locates the device by Dygma USB identity rather than a hardcoded port name.
- [x] The LED count is learned from the board's own reply, never from a constant, and printed so the 131-vs-132 question is settled on the real hardware.
- [x] The board holds a solid colour for several seconds without an active effect overwriting it.
- [x] A frames-per-second figure for full-frame writes is printed, measured rather than estimated.
- [x] Exiting restores both the previous LED mode and the baseline theme.

## Comments

Done against a wired ANSI Raise, firmware `v1.0.12-beta b3df553c f71f7f9`.

**Answers this ticket was created to get**

| Question | Answer |
| --- | --- |
| 131 or 132 LEDs? | **132.** A bare `led.theme` returns 396 values. Dygma's doc is wrong; OpenRGB's driver is right. |
| Can the wire sustain 30 fps? | **Yes, with 2.3x headroom.** 69.5 fps, 14.4 ms per frame, 1330-byte frames, flat over a 20-second soak. |
| Is fire-and-forget worth it? | **Yes.** 38 fps when waiting for each acknowledgement, 69.5 without. |
| Does `led.theme` survive an LED mode? | Only in **mode 0**. Modes 1 (rainbow), 2 (cycle) and 3 (stalker) paint over a frame within a second; 4 and above are rejected by the firmware. |

**Two facts that changed the design**

1. **The board acknowledges every command, frames included** — one `\r\n.\r\n` per frame, ~1.2 KB/s at full rate. Undrained, they fill the kernel buffer and the firmware stalls, which presents as the keyboard freezing rather than as an I/O error. The benchmark drains every 64 frames as a stopgap; ticket 07 owns making that unconditional.
2. **`dygma_focus` cannot drain**, because its read timeout is fixed at five seconds and its port is private, so one fire-and-forget frame desynchronises every later read. Discovery and the device table stay with the crate; I/O moved to `serialport`. Recorded as ADR-0004.

**Other findings**

- LED indices **20 and 131** never accept a theme colour. Both are black in the stock baseline, so nothing is lost, but a renderer assuming 132 writable LEDs is wrong and a naive "did it land" check would fail forever.
- Discovery must match `1209:2201` exactly: the crate's table also lists the Defy and Raise 2, and the bootloader `1209:2200` answers no Focus commands.

**Verification:** 40 tests (none touching the port), clippy and rustfmt clean, and the board re-read externally afterwards — mode 1, 34 distinct colours, not the frame we wrote.
