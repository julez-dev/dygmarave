# Own the serial port exclusively; Bazecor must be closed

The Dygma Raise exposes a single serial device (typically `/dev/ttyACM0`) carrying the Focus protocol — plain-text, newline-terminated commands, responses ending in a lone `.`. Bazecor uses the same port to push settings and read state. At 30 fps we write continuously, so we take the port exclusively: if the board is unplugged, or Bazecor is detected, the daemon keeps the audio pipeline alive and retries on an interval until it can write again.

## Considered Options

- **Exclusive with retry** (chosen): one writer, predictable framing, honest failure.
- **Best-effort sharing**: both writers interleave command fragments on the same stream; the firmware sees corrupt sequences and the board flickers unpredictably.
- **Detect Bazecor and refuse to start**: reliable detection is not available, and the busy-port path already covers it.

## Consequences

- Reconfiguring the board requires stopping the daemon — a documented, deliberate conflict rather than two processes fighting over one device.
- **There is no OS-level lock to lean on.** Neither Bazecor nor the `dygma_focus` crate requests exclusive access, so a port held by Bazecor never yields `EBUSY`; "exclusive" here is a matter of policy and detection, not a kernel guarantee.
- Auto-detection must tolerate `/dev/ttyACM*` renumbering across replugs, with an explicit path override in config. The `/dev/serial/by-id` fallback cannot match on the filename: it carries vendor and product *names* (`usb-Dygma_Raise_…`), which a Raise 2 or Defy also matches. Candidates are verified against sysfs `idVendor`/`idProduct` — `1209:2201` for the wired Raise — and the bootloader `1209:2200` is excluded because it answers no Focus commands.
- The retry loop and systemd's own restart policy must not both try to recover; the daemon stays alive and retries internally, so systemd restarts only on real crashes.
