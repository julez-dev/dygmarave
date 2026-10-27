# 02: Capture the sink's monitor source

**What to build:** A probe that captures the default sink's monitor and reports live level, so you can confirm the daemon hears what your speakers play rather than what your microphone hears. The failure this guards against is silent and convincing: the microphone path produces a perfectly valid stream that shows the wrong thing, so a flat or implausible signal must be reported as an error naming the source, not displayed as success.

**Blocked by:** 01 (work proceeds in numeric order)

**Status:** done

- [x] Capture follows the default output sink's monitor, not the default input device.
- [x] The probe prints live level while music plays and reads silence when playback pauses.
- [x] A stream that carries no signal, or that clearly comes from an input device, fails with a message naming the source used.
- [x] Sample rate and channel count are reported so mismatches are visible.

## Comments

Run with `cargo run -- --probe-audio [--seconds N] [--source ID]`.

**The failure was real on this machine.** `pactl` reports the default sink as the USB speakers and the default source as an **Auna CM900 microphone**, so the naive "open the default input" implementation would have visualized the room. Selection now classifies devices and prefers `pipewire:sink_default` — PipeWire's capture device for the default sink's monitor, which also means the visualizer follows whatever output the user selects rather than pinning one device.

**Evidence**

| Check | Result |
| --- | --- |
| Default sink monitor | `default_sink`, 2 ch @ 48 kHz F32, −5.7 dBFS peak while Firefox played |
| Left-only 440 Hz tone | **L −10.5 dBFS, R −100.0 dBFS** — channel separation correct |
| Idle sink (`--source` HDMI) | exit 1, "captured … but heard nothing — is anything playing, and is that sink the one it is playing to?" |
| Microphone by name (`--source "Auna Mic CM900 Mono"`) | refused: "that would visualize the room, not the speakers" |

**Findings worth carrying forward**

- Capturing a *specific* sink node dies with "Device disconnected" when that sink suspends (PipeWire suspends idle nodes). `sink_default` stayed healthy. Ticket 05 should prefer `sink_default`, and ticket 08 must reconnect when a stream dies.
- `cpal` 0.18 needs `default-features = false, features = ["pipewire", "pulseaudio"]`; its default ALSA-only path cannot reach a monitor at all.
- Stream errors are recorded into a shared slot rather than printed from the audio callback, which would otherwise write into the middle of a live meter line.
- Meter redraws in place only when stdout is a terminal; otherwise one line per second, so systemd journals stay readable.

**Not done here** (belongs to 03): no windowing, no FFT, no bands. The probe shows broadband peak, per channel.

## Review round

Two-axis review of `7823c44`. Both axes independently found the same P1, which
was real:

- **`classify` guessed from name substrings while cpal already reports the
  direction.** PipeWire exposes a sink as `DeviceDirection::Duplex` (capture it
  and you hear its output) and a microphone as `Input`. The substring rules
  missed vendor-specific names, so a Bluetooth headset microphone
  (`bluez_input.*`) classified as `Unknown` — and an explicit `--source` would
  then have captured it while printing "this is the output mix, not the
  microphone". Metadata now decides; names are only a fallback for hosts that
  report nothing.
- **Selection could be bypassed.** `Selection`'s fields were public and `start()`
  never re-checked the kind, so a hand-built selection could open a microphone.
  Fields are private with accessors, and `start()` refuses microphones as
  defence in depth.
- **The last window was thrown away.** The probe cleared its accumulator and
  `continue`d before measuring, so the final second was never counted: audio
  started near the end of `--seconds` produced exit 1 "heard nothing" for a
  source that had plainly been heard. The residual window is now measured before
  the verdict, and the verdict is a tested function rather than inline logic.
- **"Heard nothing" could not distinguish a dead stream from a paused one.** The
  message now reports how many buffers arrived, so a stream that never started
  reads differently from one that delivered silence.
- **A trailing partial frame was attributed to the wrong channels.** `level.rs`
  now walks whole frames with `chunks_exact` and ignores the orphan, with a test.
- **Errors could name a device as the empty string** when the host reports no
  description; messages now include the identifier and the kind.
- Smaller: duplicated default-sink predicate extracted; `Vec<(String, String,
  SourceKind)>` replaced with `Candidate`/`Selection`; a misleading comment about
  a "dropped block" corrected to what it is (an unbounded channel that will grow
  if ticket 05 stalls the consumer); `sample_format` is only reported now that
  non-f32 sources are refused; stale module docs and the probe-mode Ctrl-C
  wording fixed.

**Honest limit on criterion 1.** The claim is "we request the node named
`sink_default` at startup, and PipeWire maps it to the sink that is default at
that moment". A mid-run `pactl set-default-sink` was exercised and the probe kept
receiving audio, but the run was contaminated by Firefox migrating its streams
to the new sink, so it is not clean evidence of anything more than that. The
printed wording says "as chosen at startup" for that reason.
