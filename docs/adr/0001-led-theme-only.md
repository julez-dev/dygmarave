# Drive LEDs with `led.theme` only, never `palette` + `colormap.map`

The Raise's Focus API offers three ways to set LED colours: `led.theme` (a full RGB map, held in RAM), `colormap.map` + `palette` (indices into a 16-colour palette, held in **EEPROM**), and per-LED setters (`led.at`, `led.setMultiple`). A visualizer redraws 30 times a second, so we use `led.theme` exclusively. The firmware docs warn that any command with trailing data writes to EEPROM and that repeated writes "use up its lifespan" — driving a frame loop through `colormap.map` would destroy the keyboard's flash.

## Considered Options

- **`led.theme`, full frame per write** (chosen): ~1.6 KB per frame, no EEPROM interaction, no keymap side effects.
- **`palette` + `colormap.map` per frame**: smaller payload, but both live in EEPROM. Rejected: finite flash write cycles, and it would silently corrupt the user's configured per-layer colours.
- **`led.setMultiple` deltas**: repeats an index per LED, so a mostly-changed frame costs more than `led.theme`, and it needs several round trips per frame.

## Consequences

- Colour is free-form RGB, not quantised to 16 palette entries. The dark→hot ramp can be continuous.
- The daemon must not touch `palette`, `colormap.map`, `led.brightness`, or any other EEPROM-backed command in its frame loop. This is easy to regress: the smaller payload of `colormap.map` looks like an optimisation, and nothing in the code would reveal that it wears out hardware.
- `led.theme` is volatile, so the baseline theme must be read at startup and written back on exit (ADR-0002 covers the port, this covering the restore obligation).
