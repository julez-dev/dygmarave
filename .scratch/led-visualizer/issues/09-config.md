# 09: Config — XDG TOML

**What to build:** Everything that turned out to be worth tuning becomes adjustable without a rebuild: device path, audio source, decay, gate, AGC release, ramp bounds, target frame rate and the thumb settings. Config lives at the XDG location with a documented example, command-line flags override it, and bad values are rejected at startup with a clear message rather than surfacing as odd lighting mid-song.

**Blocked by:** 05

**Status:** ready-for-agent

- [ ] A config file at the XDG path is loaded, with a documented example covering every supported key.
- [ ] Command-line flags override the file for the values they set.
- [ ] Invalid or out-of-range values are rejected at startup, naming the offending key.
- [ ] Changing decay, gate, AGC release, ramp bounds or the fps target takes effect without recompiling.
