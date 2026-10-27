# Dygmarave v1 — audio-reactive LEDs on a Dygma Raise

Status: planned, not started. Decisions below are settled; facts marked **OPEN** are still being verified.

## Goal

A Linux daemon that reads the system's audio *output*, computes a spectrum, and paints it across the per-key RGB of a wired ANSI Dygma Raise in real time.

Non-goals for v1: Windows/macOS, microphone input, GUI or tray app, distro packaging, Raise 2 / Defy / wireless support, custom firmware.

Work is broken into nine tracer-bullet tickets under `.scratch/led-visualizer/issues/`, numbered in dependency order. Start at 01 and work the frontier.

## Settled decisions

| Area | Decision | Reference |
|---|---|---|
| Transport | `led.theme` full RGB frame per update, ~1.6 KB; never `palette`/`colormap.map` (EEPROM wear) | ADR-0001 |
| Port | Exclusive ownership; keep running and retry while busy/unplugged; release without restoring when Bazecor takes over | ADR-0002, ticket 08 |
| Bazecor | Detected by process check at startup and before each retry; no writing while present (there is no OS lock to rely on) | ADR-0002 |
| Serial stack | `dygma_focus` for discovery and mode vocabulary; `serialport` for I/O so acknowledgements can be drained | — |
| Ack drain | Discard buffered replies on a 250 ms wall-clock cadence; the board acks every command including frames | ticket 07 |
| LED count | Queried every startup by counting `led.theme` triples; measured 132, of which indices 20 and 131 ignore writes | — |
| LED mode | Query current mode, switch to a static non-repainting one, restore on exit (mode is EEPROM-persisted) | — |
| Frame write | Fire-and-forget, no per-frame terminator wait; a periodic read-only `led.mode` query catches a dead port (no EEPROM write) | ticket 07 |
| Layer-change flash | Accepted, not prevented: the firmware repaints on a layer change and no Focus command can stop it, so the next frame redraws over it (~33 ms at 30 fps) | — |
| Audio backlog | Drains every captured block in order each frame, so the analyzer's window stays contiguous; only a multi-second stall drops the oldest whole blocks (cap 32) | ticket 05 |
| Bands | 28 analysed; a half displays 26 (left) / 27 (right) because that is how many letter keys it has; snake, mirrored, bass at inner edges | ADR-0003 |
| Audio | System output only, PulseAudio/PipeWire monitor source via `cpal` | — |
| Analysis | 1024-sample window (≈21 ms at 48 kHz), log-spaced 40 Hz–16 kHz | — |
| Dynamics | Instant attack, slow exponential decay, per-band AGC, gate to black on silence | — |
| Colour | Continuous dark→hot perceptual ramp (black → red → orange → white) | — |
| Thumbs | Per-channel level glow per half; white flash on bass onset | — |
| Rate | 30 fps target; measured ceiling 69.5 fps, adaptive downward — floor 5 fps, step a sixth of the target, two resyncs or <90% achieved in a 2 s window to degrade, 6 s clean to recover; the budget is typing latency, not the wire | ticket 07 |
| Lifecycle | Read baseline theme at startup, write it back on clean exit; on SIGTERM/SIGINT restore and exit cleanly; systemd user unit at login with `Restart=on-failure` so its policy does not fight the daemon's internal retry | ticket 08 |
| Config | XDG TOML at `~/.config/dygmarave/config.toml`, CLI overrides | — |
| Discovery | `/dev/serial/by-id` match on Dygma VID/PID, explicit path override | — |
| Scope | `cargo run` + systemd **user** unit, Linux only | — |

Vocabulary is defined in `/GLOSSARY.md`.

## Architecture

Six modules, each with a narrow interface:

- `device` — discovery (VID/PID `1209:2200`/`1209:2201` or explicit path), Bazecor presence check, mode query/switch/restore, LED-count query, frame write, baseline read/restore. Knows the wire format, nothing about audio.
- `audio` — capture from a monitor source, emit fixed-size f32 blocks with a sample rate. Knows nothing about LEDs.
- `analysis` — windowed FFT, 28 log-spaced bands per channel, AGC, attack/decay, gate, bass onset detection. Pure function of audio blocks → `Frame` values.
- `render` — `Frame` → 131-entry RGB map. Owns the ramp, the band→LED table (snake + mirror), thumbs and the beat flash.
- `config` — TOML load, CLI override, validation/defaults.
- `main`/`daemon` — wiring, 30 fps pacing loop, adaptive rate, retry/backoff, Bazecor pause, signal handling, baseline restore. The outer loop keeps the audio pipeline alive across port loss and Bazecor; the inner loop writes frames until interrupted, Bazecor appears, or the port fails.

Data flow:

```
monitor source → [audio] → blocks → [analysis] → Frame{bands[2][28], level[2], beat:bool}
                                                        ↓
                                            [render] → [u8; 3 * 131]
                                                        ↓
                                             [device] → led.theme …\n
```

Three hard constraints inherited from the firmware:

1. `palette`, `colormap.map`, `keymap.*` and `led.brightness` are EEPROM-backed. `led.theme` is RAM-only — the THEME handler in `LEDControl.cpp` calls `setCrgbAt()` and nothing else. The frame loop may only emit `led.theme`.
2. The wire format is **plain-text, space-separated decimal, not JSON**: `led.theme R G B R G B …\n`, one triplet per LED index. Responses are terminated by a line containing only `.`.
3. An **active LED mode repaints every main-loop cycle** and overwrites whatever `led.theme` drew. The daemon must select a non-repainting mode before its first frame, and restore the previous mode on exit.

## Config sketch

```toml
[device]
port = "auto"            # or /dev/serial/by-id/...

[audio]
source = ""              # empty = default sink's monitor

[analysis]
window = 1024
band_min_hz = 40.0
band_max_hz = 16000.0

[render]
fps_target = 30
attack = "instant"
decay_ms = 180
gate_db = -55.0
agc_release_ms = 4000
target = 0.7             # AGC ceiling; pure white stays at 1.0, so loud never clips to white
max_gain = 100.0         # 40 dB, enough for a track mastered at the -55 dBFS gate
floor = 0.02             # below this a decaying band is drawn black, so silence goes dark

[thumbs]
level = true
beat_flash = true
beat_threshold = 2.0     # multiple of rolling average energy
```

## Prior art

Worth copying rather than inventing: OpenRGB's `DygmaRaiseController` (C++) drives all LEDs through bulk `led.theme` writes, fire-and-forget without reading the `.` terminator, and closes the port on teardown. Paired with KeyboardVisualizer it already forms an audio-reactive pipeline on this board — so the risk in this project is the DSP and lifecycle, not the transport. In Rust, the `dygma_focus` crate (v0.5.1) wraps discovery and most Focus commands, but has no `led.theme` helper, so the hot path may still need raw writes. No Rust Dygma audio visualizer exists.

## Milestones

**M1 — Vertical slice.** Open the port, query the LED count and current mode, switch to a static mode, read the baseline theme, write a solid colour, then measure how many full `led.theme` frames per second the wire and firmware actually sustain. Proves the riskiest assumption — ADR-0001's per-frame cost and the spec's 30 fps target — before any DSP exists. Also settles 131-vs-132 on the real board. If the measured rate lands near the low end of the 10–40 fps estimate, this is the moment to revisit, not after six modules are built.

**M2 — Audio pipeline.** Capture the monitor source, window and FFT it, fold into 28 log-spaced bands per channel. Verify by dumping band energies to stdout while playing known tones.

**M3 — Mapping.** Build the band→LED-index table for the ANSI Raise (snake, mirrored halves) and render a static gradient across the board to confirm geometry visually.

**M4 — Dynamics and colour.** AGC, attack/decay, gate, ramp, thumb level and beat flash. Tune by ear and eye against real music.

**M5 — Lifecycle.** Retry/backoff on busy or missing port, baseline restore on exit and on SIGTERM, systemd user unit, config file with validation.

## Facts — measured on hardware

Measured on 2026-10-27 against a wired ANSI Raise, firmware `v1.0.12-beta b3df553c f71f7f9`, by ticket 01's tool.

- **132 LEDs.** A bare `led.theme` returns 396 values, so OpenRGB's figure was right and Dygma's doc (131) was wrong. Still queried at runtime rather than hardcoded: `Cargo.toml`'s count is whatever the board says.
- **Two indices never accept a theme colour: 20 and 131.** They remain black through a solid-colour write and through restore. Both are black in the stock baseline, so nothing is lost, but a renderer that assumes 132 writable LEDs is wrong, and a naive "did the frame land" check would fail forever.
- **Throughput is 69.5 fps** for full 1330-byte frames, 14.4 ms per frame, 90 KiB/s — steady over a 20-second soak. The research estimate of 10–40 fps was too pessimistic. The 30 fps target has ~2.3× headroom, so it stays where it is: fps is now limited by the latency budget, not the wire.
- **Waiting for the ack costs half the rate.** Request/response per frame measured 38 fps (26 ms/frame) versus 69.5 fire-and-forget. Fire-and-forget is confirmed, not assumed.
- **The board acks every command, frames included** — one `\r\n.\r\n` per frame, ≈1.2 KB/s at full rate. Unread, they fill the kernel buffer and stall the firmware. Ticket 01 drains every 64 frames.
- **`led.mode` values are 0–3, and only 0 is static.** Measured: mode 0 holds a written frame indefinitely; 1 (rainbow), 2 (cycle) and 3 (stalker) paint over it within a second; 4 and above are rejected by the firmware (which contradicts the `dygma_focus` enum, which lists Red/Green/Blue as 4–6).
- **`dygma_focus` cannot implement the hot path.** It hard-codes a 5 s read timeout and keeps its port private, so acknowledgements can neither be peeked nor discarded; a read after any fire-and-forget frame returns a stale ack. Discovery and the mode vocabulary still come from it.
- **The predicted audio failure is real on this machine.** `pactl` reports the default *sink* as the USB speakers and the default *source* as the Auna CM900 microphone, so opening the default input would visualize the room. Measured: cpal's PipeWire host offers `pipewire:sink_default` (the default sink's monitor, which follows the user's output choice), every sink as a capturable node, `pipewire:input_default` (the microphone), and the hardware inputs. `cpal` 0.18 needs `--no-default-features --features pipewire,pulseaudio`; the ALSA path can only reach the microphone.
- **Capturing a specific sink node dies when that sink suspends.** Measuring an idle HDMI sink produced "Device disconnected" on the stream while `sink_default` stayed healthy, because PipeWire suspends idle nodes. Anything that pins one sink needs reconnect handling (ticket 08).
- **The spectrum is measured, not assumed.** A 12-second log sweep (60 Hz → 15 kHz) played into a sink and read back through the monitor moved the peak band **6 → 8 → 10 → 12 → 14 → 16 → 19 → 21 → 23 → 25 → 27**, strictly monotonic and ending in the top band; a 1 kHz tone landed in band 15 (~1109 Hz). Amplitude reads true: a 0.5 sine measures ~0.5 after correcting for the Hann window's 1.5-bin noise bandwidth.
- **Bass is coarser than one band.** 1024 samples at 48 kHz gives 46.9 Hz bins, wider than the lowest bands (40–62 Hz), so a tone below ~150 Hz legitimately straddles neighbours and cannot be assigned to a single band. Acceptable for a display where bass is a blur, but it rules out precise bass banding without a longer window (which would cost transient response).
- **The physical grid is 5×16 with 68 key LEDs**, verified by photograph: left half columns 0–7 (LEDs 0–32), right half columns 8–15 (LEDs 33–68), mirrored about the centre gap. LED 40 sits on no key, and **LED 20 — the `\` key — is dead on this unit**.
- **Letter rows hold 26 keys on the left and 27 on the right**, not 28, because the physical grid has holes. Bands 26/27 (left) fall off the end.
- **Only a layer change repaints the board**, not ordinary typing. A 30 fps loop hides it within one frame, so it is a cosmetic flash rather than a blocker.
- **The physical key map is Bazecor's, not the firmware's.** Verified on hardware: LED 0 is Escape (top-left) and LED 26 is left Ctrl (bottom-left), matching Bazecor's 5×16 physical map and contradicting the firmware's 4×16 keymap map. Bazecor also reports **69 key LEDs (0–68)** with underglow from 69, so the earlier "64 keys" figure was wrong.
- **A layer change repaints the whole board**, wiping a written frame. A 30 fps loop would restore it within a frame, but this is the first sign that written frames are less durable than assumed; whether ordinary typing does it too is unresolved.
- **Channel separation is correct end to end.** A 440 Hz tone written to the left channel only measured **L −10.5 dBFS, R −100.0 dBFS** through the monitor, so the interleaved-sample maths can be trusted for per-half colours.
- **No OS-level exclusive lock exists.** Neither Bazecor nor `dygma_focus` requests one (`set_exclusive(false)`), so a busy port never raises `EBUSY` and the retry path cannot detect Bazecor on its own — hence the process check.

## Known open items

- **Index→key table** must be built by correlating the runtime LED order against `RaiseANSIColorMap.png`; no machine-readable table exists upstream. Ticket 04.
- **Ack-drain cadence** is now wall-clock based in ticket 07 (every 250 ms), and the health probe queries every 10 s. A real-board soak is still needed to prove the port never stalls over an hour-long show.
- **Per-sink capture and sink suspension** — ticket 05 prefers `sink_default`, which stays healthy when idle sinks suspend. A per-sink node still needs its own reconnect handling; ticket 08's port-level retry covers the board, not the audio node.

## Risks

- **Firmware parser throughput.** ~396 decimal integers per frame is the dominant cost, and `syncLeds()` sends each half's data four times. Measured headroom is comfortable (69.5 fps against a 30 fps target), so this is no longer a rate risk — the remaining rate risk is typing latency, below.
- **Unread acknowledgements.** The board replies to every command, including every frame. Without periodic draining the kernel buffer fills and the firmware stalls mid-show — silent until it freezes. Ticket 01 drains in the benchmark; ticket 07 must make it unconditional in the render loop.
- **Typing latency.** Dygma's own issue tracker reports LED effects substantially increasing key-to-USB latency (Keyboardio measured 9–14 ms under a heavy LED effect). This daemon writes continuously, so it will cost typing latency; that is a known price of the feature, not a bug to chase.
- **EEPROM wear.** Any accidental use of an EEPROM-backed command in the frame loop. Mitigated by routing all writes through one function and never exposing the others.
- **USB autosuspend / replug.** Port may vanish mid-show; retry loop must reconstruct state, including re-reading the baseline theme.
- **AGC pumping.** Per-band AGC can amplify the noise floor in quiet passages; the gate is the mitigation and `agc_release_ms` the tuning knob.
- **Wrong audio source.** Silently capturing the microphone instead of the sink's monitor is the single most likely "it runs but shows the wrong thing" failure.
