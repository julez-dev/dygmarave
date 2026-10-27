# Own the serial I/O directly; use `dygma_focus` only for discovery

The Raise answers *every* command with `\r\n.\r\n`, including every `led.theme` frame. At the measured 69.5 frames per second that is roughly 1.2 KB/s of acknowledgements nobody is waiting for. They arrive on the same USB endpoint as real replies, so the only way to keep asking the board questions mid-show is to periodically read and discard them. `dygma_focus::Focus` hard-codes a five-second read timeout and keeps its port private, so it can neither peek at nor discard that backlog. A single fire-and-forget frame therefore desynchronises every subsequent read: the next `query` returns the stale acknowledgement instead of its answer. So we keep `dygma_focus` for what it is good at — matching the Raise's USB vendor and product IDs during discovery, and naming the LED modes — and do the reading and writing through `serialport` ourselves.

## Considered Options

- **`dygma_focus` for I/O, pay an ack per frame** (rejected): drain by calling `read_string()` once per outstanding frame. Works only while the count of dropped acks is exactly zero; the first mismatch blocks for five seconds, and there is no way to ask "is anything waiting?".
- **`dygma_focus` for I/O, wait for every ack** (rejected): measured at 38 fps against 69.5 fire-and-forget. It also throws away the only signal that a frame was applied, since the ack doubles as an apply barrier we do not need.
- **`serialport` for I/O, `dygma_focus` for discovery** (chosen): about forty lines of framing, a one-millisecond drain timeout, and full control over the read timeout.
- **`serialport` for everything** (rejected): re-implementing USB VID/PID matching and the mode vocabulary for no gain.

## Consequences

- The transport owns its own timeout discipline: five seconds when waiting for an answer, one millisecond when draining, and any drain must restore the patient timeout afterwards.
- Acknowledgements have to be drained on a cadence forever, not once. A long show that stops draining fills the kernel buffer and stalls the firmware — which presents as the keyboard freezing, not as an I/O error.
- A query must skip empty replies, because an acknowledgement for an earlier frame can arrive after the drain and before the answer. Any query whose answer is legitimately empty will block for the full five seconds; only known-good getters may be called.
- Discovery stays with the crate, so the transport module is now the only file that knows either library exists.
