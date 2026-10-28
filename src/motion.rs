//! Additive motion: a modulation layered over the composed matrix.
//!
//! Motion never chooses colours or places bands; it only brightens or dims the
//! colours the layout and palette already produced. The thumb clusters are
//! deliberately outside this module: they keep their palette level glow and white
//! beat flash, and no motion may touch them.
//!
//! [`MotionState::advance`] is the only place event state changes. Rendering is
//! read-only over the state, so a crossfade can compose two selections in one
//! frame without spawning sparkles or advancing ripples twice.

use std::fmt;
use std::str::FromStr;
use std::time::Instant;

use crate::layout::{self, Cell, Half};
use crate::render::Dynamics;
use crate::theme::Rgb;

const TAU: f32 = std::f32::consts::TAU;
const HALVES: usize = 2;

const SWEEP_PERIOD_S: f32 = 3.0;
const ORBIT_PERIOD_S: f32 = 4.0;
const GLINT_LIFE_S: f32 = 0.12;
const GLINT_TAU_S: f32 = 0.035;
const RIPPLE_LIFE_S: f32 = 0.6;
const RAIN_FALL_ROWS_PER_S: f32 = 3.0;
const MIN_BRIGHTNESS: f32 = 0.3;

const MAX_BEATS_PER_ADVANCE: u64 = 8;
const SPARKLE_SLOTS: usize = 16;
const RIPPLE_SLOTS: usize = 2;
const RAIN_DROPS: usize = 4;

const MAX_INTENSITY: f32 = 2.0;
const MIN_SPEED: f32 = 0.25;
const MAX_SPEED: f32 = 4.0;

/// The name of a motion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Motion {
    #[default]
    None,
    Sweep,
    SweepVertical,
    Sparkle,
    Breathe,
    Ripple,
    Orbit,
    Rain,
}

impl Motion {
    /// Every motion, in the order the spec lists them.
    pub const ALL: [Motion; 8] = [
        Motion::None,
        Motion::Sweep,
        Motion::SweepVertical,
        Motion::Sparkle,
        Motion::Breathe,
        Motion::Ripple,
        Motion::Orbit,
        Motion::Rain,
    ];

    /// The config spelling of every motion.
    pub const NAMES: [&'static str; 8] = [
        "none",
        "sweep",
        "sweep-vertical",
        "sparkle",
        "breathe",
        "ripple",
        "orbit",
        "rain",
    ];

    /// The config spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Motion::None => "none",
            Motion::Sweep => "sweep",
            Motion::SweepVertical => "sweep-vertical",
            Motion::Sparkle => "sparkle",
            Motion::Breathe => "breathe",
            Motion::Ripple => "ripple",
            Motion::Orbit => "orbit",
            Motion::Rain => "rain",
        }
    }

    /// Parses a config spelling, case-sensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|motion| motion.name() == name)
    }
}

impl fmt::Display for Motion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a motion name was not recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownMotion {
    pub name: String,
}

impl fmt::Display for UnknownMotion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown motion {:?}; expected one of {}",
            self.name,
            Motion::NAMES.join(", ")
        )
    }
}

impl std::error::Error for UnknownMotion {}

impl FromStr for Motion {
    type Err = UnknownMotion;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_name(name).ok_or_else(|| UnknownMotion {
            name: name.to_string(),
        })
    }
}

/// The motion axis's configurable selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionSettings {
    pub motion: Motion,
    pub intensity: f32,
    pub speed: f32,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self {
            motion: Motion::None,
            intensity: 1.0,
            speed: 1.0,
        }
    }
}

impl MotionSettings {
    /// Clamps the numeric knobs defensively.
    ///
    /// Ticket 04's config validation owns user input; these debug assertions make
    /// a bad internal call site fail loudly in tests and debug builds instead of
    /// letting a NaN propagate through the modulation maths.
    fn normalised(self) -> Self {
        debug_assert!(
            self.intensity.is_finite() && (0.0..=MAX_INTENSITY).contains(&self.intensity),
            "motion intensity must be finite and in 0..=2, got {}",
            self.intensity
        );
        debug_assert!(
            self.speed.is_finite() && (MIN_SPEED..=MAX_SPEED).contains(&self.speed),
            "motion speed must be finite and in 0.25..=4, got {}",
            self.speed
        );
        Self {
            motion: self.motion,
            intensity: self.intensity.clamp(0.0, MAX_INTENSITY),
            speed: self.speed.clamp(MIN_SPEED, MAX_SPEED),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Glint {
    cell: u8,
    age: f32,
    strength: f32,
    active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Ripple {
    age: f32,
    active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Droplet {
    depth: u8,
    phase: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Rain {
    started: f32,
    drops: [Droplet; RAIN_DROPS],
}

impl Rain {
    fn draw(rng: &mut fastrand::Rng, started: f32) -> Self {
        let mut drops = [Droplet::default(); RAIN_DROPS];
        for drop in &mut drops {
            drop.depth = rng.usize(0..layout::DEPTH_COUNT) as u8;
            drop.phase = rng.f32() * 5.0;
        }
        Self { started, drops }
    }
}

/// The live motion: selection plus the clock and event state it needs.
///
/// Periodic motions read one continuous `elapsed` clock. Sparkle glints and
/// ripple rings are event state advanced by [`MotionState::advance`]; rain
/// droplets are deterministic event state re-drawn by
/// [`MotionState::reset_events`]. A reset clears events but never resets the
/// clock.
pub struct MotionState {
    settings: MotionSettings,
    rng: fastrand::Rng,
    elapsed: f32,
    last_advance: Option<Instant>,
    /// The last beat count observed per half. `None` means "sync on the next
    /// advance without firing", which suppresses a beat that happened before a
    /// reset or motion switch.
    seen_beats: [Option<u64>; HALVES],
    glints: [[Glint; SPARKLE_SLOTS]; HALVES],
    next_glint: [usize; HALVES],
    ripples: [[Ripple; RIPPLE_SLOTS]; HALVES],
    rain: [Rain; HALVES],
}

impl MotionState {
    pub fn new(settings: MotionSettings, seed: u64) -> Self {
        let settings = settings.normalised();
        let mut state = Self {
            settings,
            rng: fastrand::Rng::with_seed(seed),
            elapsed: 0.0,
            last_advance: None,
            seen_beats: [None; HALVES],
            glints: [[Glint::default(); SPARKLE_SLOTS]; HALVES],
            next_glint: [0; HALVES],
            ripples: [[Ripple::default(); RIPPLE_SLOTS]; HALVES],
            rain: [Rain::default(); HALVES],
        };
        state.reset_events();
        state
    }

    /// The selection currently being rendered.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settings(&self) -> MotionSettings {
        self.settings
    }

    /// Test-only view of the continuous clock, used by the look engine's switch
    /// tests to prove periodic phase survives a motion change.
    #[cfg(test)]
    pub(crate) fn elapsed_seconds(&self) -> f32 {
        self.elapsed
    }

    /// Test-only count of active sparkle/ripple events, used to prove a switch
    /// clears event state.
    #[cfg(test)]
    pub(crate) fn active_event_count(&self) -> usize {
        self.glints
            .iter()
            .flatten()
            .filter(|glint| glint.active)
            .count()
            + self
                .ripples
                .iter()
                .flatten()
                .filter(|ripple| ripple.active)
                .count()
    }

    /// Replaces the selection for startup/test setup. Live switches must use
    /// [`MotionState::set_settings_at`] so the beat counter is snapshotted.
    #[cfg(test)]
    pub fn set_settings(&mut self, settings: MotionSettings) {
        let settings = settings.normalised();
        let motion_changed = settings.motion != self.settings.motion;
        self.settings = settings;
        if motion_changed {
            self.reset_events();
        }
    }

    /// [`MotionState::set_settings`] for a live switch: snapshots the current beat
    /// counts so a beat after the switch but before the next advance still fires.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_settings_at(&mut self, settings: MotionSettings, dynamics: &Dynamics) {
        let settings = settings.normalised();
        let motion_changed = settings.motion != self.settings.motion;
        self.settings = settings;
        if motion_changed {
            self.reset_events_at(dynamics);
        }
    }

    /// Switches motion for startup/test setup. Live switches must use
    /// [`MotionState::set_motion_at`] so the beat counter is snapshotted.
    #[cfg(test)]
    pub fn set_motion(&mut self, motion: Motion) {
        if motion != self.settings.motion {
            self.settings.motion = motion;
            self.reset_events();
        }
    }

    /// [`MotionState::set_motion`] for a live switch. See
    /// [`MotionState::reset_events_at`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_motion_at(&mut self, motion: Motion, dynamics: &Dynamics) {
        if motion != self.settings.motion {
            self.settings.motion = motion;
            self.reset_events_at(dynamics);
        }
    }

    /// Clears event state for construction/startup, leaving the clock untouched.
    ///
    /// This deliberately suppresses a beat until the next advance snapshots the
    /// counters. A beat that happens after this call but before that advance is
    /// lost. Switch paths must use [`MotionState::reset_events_at`] instead.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn reset_events(&mut self) {
        self.clear_events();
        self.seen_beats = [None; HALVES];
    }

    /// Clears event state for a live switch and snapshots the current beat counts.
    ///
    /// Call this with the same `Dynamics` the frame will use, before that frame's
    /// [`MotionState::advance`]. Only beats the counters gain *after* this call
    /// spawn events; beats already present cannot fire late.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn reset_events_at(&mut self, dynamics: &Dynamics) {
        self.clear_events();
        for half in 0..HALVES {
            self.seen_beats[half] = Some(dynamics.beat_count(channel_for_half(dynamics, half)));
        }
    }

    fn clear_events(&mut self) {
        self.glints = [[Glint::default(); SPARKLE_SLOTS]; HALVES];
        self.next_glint = [0; HALVES];
        self.ripples = [[Ripple::default(); RIPPLE_SLOTS]; HALVES];
        // Rain is redrawn from the continuing RNG, so repeated switches do not
        // replay one frozen pattern.
        for half in 0..HALVES {
            let rain = Rain::draw(&mut self.rng, self.elapsed);
            self.rain[half] = rain;
        }
    }

    /// Advances the clock and event state by the time since the previous call.
    ///
    /// Call this exactly once per frame, before composing any selection that
    /// reads this state. It is the only place sparkle/ripple events are spawned or
    /// aged.
    pub fn advance(&mut self, now: Instant, dynamics: &Dynamics) {
        let dt = match self.last_advance {
            Some(previous) => now.saturating_duration_since(previous).as_secs_f32(),
            None => 0.0,
        };
        self.last_advance = Some(now);
        let dt = dt.max(0.0);
        self.elapsed += dt;
        self.age_events(dt);

        for half in 0..HALVES {
            let channel = channel_for_half(dynamics, half);
            let count = dynamics.beat_count(channel);
            let previous = self.seen_beats[half];
            self.seen_beats[half] = Some(count);

            // Construction/startup leaves `previous` as None: snapshot the
            // counter without firing. A live switch uses `reset_events_at` and
            // supplies a real snapshot instead.
            let Some(previous) = previous else {
                continue;
            };
            let hits = count.saturating_sub(previous).min(MAX_BEATS_PER_ADVANCE);
            for _ in 0..hits {
                match self.settings.motion {
                    Motion::Sparkle => self.spawn_sparkle(half),
                    Motion::Ripple => self.spawn_ripple(half),
                    _ => {}
                }
            }
        }
    }

    fn age_events(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        for half in 0..HALVES {
            for glint in &mut self.glints[half] {
                if !glint.active {
                    continue;
                }
                glint.age += dt;
                if glint.age >= GLINT_LIFE_S {
                    *glint = Glint::default();
                }
            }
        }
        for half in 0..HALVES {
            for ripple in &mut self.ripples[half] {
                if !ripple.active {
                    continue;
                }
                ripple.age += dt;
                if ripple.age >= RIPPLE_LIFE_S {
                    *ripple = Ripple::default();
                }
            }
        }
    }

    fn spawn_sparkle(&mut self, half: usize) {
        if self.settings.intensity <= 0.0 {
            return;
        }
        // Count scales with intensity; one is still a glint at low intensity.
        let count = ((1.0 + self.settings.intensity * 4.0).round() as usize).min(SPARKLE_SLOTS);
        for _ in 0..count {
            let Some(cell) = self.random_present_cell(half) else {
                break;
            };
            let slot = self.next_glint[half] % SPARKLE_SLOTS;
            self.next_glint[half] = (slot + 1) % SPARKLE_SLOTS;
            self.glints[half][slot] = Glint {
                cell: cell.index() as u8,
                age: 0.0,
                strength: 0.55 + 0.45 * self.rng.f32(),
                active: true,
            };
        }
    }

    fn random_present_cell(&mut self, half: usize) -> Option<Cell> {
        let half = if half == 0 { Half::Left } else { Half::Right };
        // Rejection sampling over the 4x7 rectangle. At least 26/28 cells are
        // present, so 64 attempts cannot miss in practice.
        for _ in 0..64 {
            let index = self.rng.usize(0..layout::CELL_COUNT);
            let cell = Cell::from_index(index)?;
            if cell.led(half).is_some() {
                return Some(cell);
            }
        }
        None
    }

    fn spawn_ripple(&mut self, half: usize) {
        if self.settings.intensity <= 0.0 {
            return;
        }
        let mut slot = 0;
        let mut oldest = -1.0;
        for (index, ripple) in self.ripples[half].iter().enumerate() {
            if !ripple.active {
                slot = index;
                break;
            }
            if ripple.age > oldest {
                oldest = ripple.age;
                slot = index;
            }
        }
        self.ripples[half][slot] = Ripple {
            age: 0.0,
            active: true,
        };
    }

    /// The colour one matrix key should show after motion.
    ///
    /// Read-only: this may be called twice in one frame for a crossfade without
    /// changing any event state. The caller paints thumb LEDs separately, so this
    /// function is only ever reached for matrix keys.
    pub(crate) fn modulate(&self, colour: Rgb, cell: Cell, half: Half, dynamics: &Dynamics) -> Rgb {
        if self.settings.motion == Motion::None || self.settings.intensity == 0.0 {
            return colour;
        }

        let half_index = half_index(half);
        let channel = channel_for_half(dynamics, half_index);
        match self.settings.motion {
            Motion::None => colour,
            Motion::Sweep => scale(
                colour,
                self.sweep_factor(cell.depth() as f32 / (layout::DEPTH_COUNT - 1) as f32),
            ),
            Motion::SweepVertical => scale(
                colour,
                self.sweep_factor(cell.row() as f32 / (layout::ROW_COUNT - 1) as f32),
            ),
            Motion::Breathe => scale(colour, self.breathe_factor(dynamics.thumb_level(channel))),
            Motion::Sparkle => blend_white(colour, self.sparkle_blend(half_index, cell.index())),
            Motion::Ripple => blend_white(colour, self.ripple_blend(half_index, cell.depth())),
            Motion::Orbit => blend_white(colour, self.orbit_blend(cell)),
            Motion::Rain => blend_white(colour, self.rain_blend(half_index, cell)),
        }
    }

    /// Crest brightness for a normalised position along the sweep axis.
    fn sweep_factor(&self, position: f32) -> f32 {
        let period = SWEEP_PERIOD_S / self.settings.speed;
        let phase = (self.elapsed / period).rem_euclid(1.0);
        let crest = 0.5 * (1.0 + (TAU * (position - phase)).cos());
        (1.0 - 0.6 * self.settings.intensity * (1.0 - crest)).max(MIN_BRIGHTNESS)
    }

    fn breathe_factor(&self, level: f32) -> f32 {
        let intensity = self.settings.intensity;
        (1.0 - 0.35 * intensity + 0.35 * intensity * level.clamp(0.0, 1.0))
            .clamp(MIN_BRIGHTNESS, 1.0)
    }

    fn sparkle_blend(&self, half: usize, cell_index: usize) -> f32 {
        let mut best = 0.0f32;
        for glint in &self.glints[half] {
            if !glint.active || glint.cell as usize != cell_index {
                continue;
            }
            let decay = (-glint.age / GLINT_TAU_S).exp();
            best = best.max(glint.strength * decay);
        }
        (best * self.settings.intensity * 0.9).clamp(0.0, 1.0)
    }

    fn ripple_blend(&self, half: usize, depth: usize) -> f32 {
        let extent = (layout::DEPTH_COUNT - 1) as f32;
        let mut best = 0.0f32;
        for ripple in &self.ripples[half] {
            if !ripple.active {
                continue;
            }
            let progress = (ripple.age / RIPPLE_LIFE_S).clamp(0.0, 1.0);
            if progress >= 1.0 {
                continue;
            }
            let radius = progress * extent;
            let ring = (1.0 - (depth as f32 - radius).abs() / 1.2).max(0.0);
            let fade = 1.0 - progress;
            best = best.max(ring * fade);
        }
        (best * self.settings.intensity * 0.85).clamp(0.0, 1.0)
    }

    fn orbit_blend(&self, cell: Cell) -> f32 {
        let period = ORBIT_PERIOD_S / self.settings.speed;
        let phase = (self.elapsed / period).rem_euclid(1.0);
        let theta = TAU * phase;
        // One spot in normalised coordinates; mirroring falls out of the shared
        // coordinate system.
        let depth_centre = 3.0 + 2.4 * theta.cos();
        let row_centre = 1.5 + 1.2 * theta.sin();
        let depth_delta = cell.depth() as f32 - depth_centre;
        let row_delta = (cell.row() as f32 - row_centre) * 1.6;
        let distance = (depth_delta * depth_delta + row_delta * row_delta).sqrt();
        let spot = (1.0 - distance / 2.2).max(0.0);
        (self.settings.intensity * 0.7 * spot).clamp(0.0, 1.0)
    }

    fn rain_blend(&self, half: usize, cell: Cell) -> f32 {
        let rain = &self.rain[half];
        let t = (self.elapsed - rain.started).max(0.0);
        let mut best = 0.0f32;
        for drop in &rain.drops {
            if drop.depth != cell.depth() as u8 {
                continue;
            }
            // A drop cycles from above the top row to below the bottom row.
            let y =
                (drop.phase + t * self.settings.speed * RAIN_FALL_ROWS_PER_S).rem_euclid(5.0) - 1.0;
            let distance = (cell.row() as f32 - y).abs();
            best = best.max((1.0 - distance / 0.8).max(0.0));
        }
        (best * self.settings.intensity * 0.8).clamp(0.0, 1.0)
    }
}

impl Default for MotionState {
    fn default() -> Self {
        Self::new(MotionSettings::default(), 0)
    }
}

fn half_index(half: Half) -> usize {
    match half {
        Half::Left => 0,
        Half::Right => 1,
    }
}

fn channel_for_half(dynamics: &Dynamics, half: usize) -> usize {
    if dynamics.channels() == 1 {
        0
    } else {
        half.min(dynamics.channels().saturating_sub(1))
    }
}

fn blend_white(colour: Rgb, amount: f32) -> Rgb {
    colour.lerp(Rgb::new(255, 255, 255), amount)
}

fn scale(colour: Rgb, factor: f32) -> Rgb {
    let factor = factor.max(0.0);
    let channel = |value: u8| (value as f32 * factor).round().clamp(0.0, 255.0) as u8;
    Rgb::new(channel(colour.r), channel(colour.g), channel(colour.b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::render::{compose, DynamicsConfig, Look, BASS_BANDS};
    use crate::spectrum::{Spectrum, BAND_COUNT};

    fn spectrum_with(values: &[f32]) -> Spectrum {
        let mut spectrum = Spectrum::new(1, values.len());
        spectrum.set_bands(0, values);
        spectrum
    }

    fn dynamics_with_bands(values: &[f32]) -> Dynamics {
        let mut dynamics = Dynamics::new(DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectrum_with(values), 1.0 / 30.0);
        dynamics
    }

    fn quiet_dynamics() -> Dynamics {
        dynamics_with_bands(&[0.0; BAND_COUNT])
    }

    fn beat_spectrum() -> Spectrum {
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..BASS_BANDS].fill(0.8);
        spectrum_with(&bands)
    }

    fn settings(motion: Motion, intensity: f32, speed: f32) -> MotionSettings {
        MotionSettings {
            motion,
            intensity,
            speed,
        }
    }

    fn active_glints(state: &MotionState, half: usize) -> usize {
        state.glints[half]
            .iter()
            .filter(|glint| glint.active)
            .count()
    }

    fn active_ripples(state: &MotionState, half: usize) -> usize {
        state.ripples[half]
            .iter()
            .filter(|ripple| ripple.active)
            .count()
    }

    #[test]
    fn every_motion_name_round_trips() {
        assert_eq!(Motion::ALL.len(), 8);
        assert_eq!(Motion::NAMES.len(), 8);
        for name in Motion::NAMES {
            let motion: Motion = name.parse().expect("the name must parse");
            assert_eq!(motion.name(), name);
            assert_eq!(motion.to_string(), name);
            assert_eq!(Motion::from_name(name), Some(motion));
        }
        assert_eq!(
            "nope".parse::<Motion>(),
            Err(UnknownMotion {
                name: "nope".to_string()
            })
        );
        assert_eq!(Motion::default(), Motion::None);
        assert_eq!(MotionSettings::default(), settings(Motion::None, 1.0, 1.0));
    }

    #[test]
    fn every_motion_is_identity_at_zero_intensity() {
        let mut bands = vec![0.0; BAND_COUNT];
        for (band, energy) in bands.iter_mut().enumerate() {
            *energy = 0.001 * (band + 1) as f32;
        }
        let dynamics = dynamics_with_bands(&bands);
        let look = Look::default();
        let start = Instant::now();

        let mut baseline = MotionState::new(MotionSettings::default(), 99);
        baseline.advance(start, &dynamics);
        let expected = compose(&dynamics, 132, &look, &baseline);

        for motion in Motion::ALL {
            let mut state = MotionState::new(settings(motion, 0.0, 1.0), 99);
            state.advance(start, &dynamics);
            let frame = compose(&dynamics, 132, &look, &state);
            assert_eq!(frame, expected, "{motion} moved the frame at intensity 0");
        }
    }

    #[test]
    fn sweep_and_vertical_sweep_use_their_own_axes() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();

        let mut sweep = MotionState::new(settings(Motion::Sweep, 1.0, 1.0), 1);
        sweep.advance(start, &dynamics);
        let inner = Cell::from_index(0).unwrap(); // (row 0, depth 0)
        let outer = Cell::from_index(3).unwrap(); // (row 0, depth 3)
        let grey = Rgb::new(120, 120, 120);
        let inner = sweep.modulate(grey, inner, Half::Left, &dynamics);
        let outer = sweep.modulate(grey, outer, Half::Left, &dynamics);
        assert!(
            inner.r > outer.r,
            "the sweep crest should start at the inner edge: {inner:?} vs {outer:?}"
        );

        let mut vertical = MotionState::new(settings(Motion::SweepVertical, 1.0, 1.0), 1);
        vertical.advance(start, &dynamics);
        let top_cell = Cell::from_index(0).unwrap(); // (row 0, depth 0)
        let second_row_cell = Cell::from_index(layout::DEPTH_COUNT).unwrap(); // (row 1, depth 0)
        let top = vertical.modulate(grey, top_cell, Half::Left, &dynamics);
        let second_row = vertical.modulate(grey, second_row_cell, Half::Left, &dynamics);
        assert!(
            top.r > second_row.r,
            "the vertical crest should start at the top: {top:?} vs {second_row:?}"
        );

        // Half a period later the crest has moved down to the second row.
        vertical.advance(start + Duration::from_millis(1_500), &dynamics);
        let top_now = vertical.modulate(grey, top_cell, Half::Left, &dynamics);
        let second_row_now = vertical.modulate(grey, second_row_cell, Half::Left, &dynamics);
        assert!(
            second_row_now.r > top_now.r,
            "the vertical crest must travel top to bottom"
        );
    }

    #[test]
    fn sweep_crest_travels_with_its_documented_period() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let mut state = MotionState::new(settings(Motion::Sweep, 1.0, 1.0), 1);
        state.advance(start, &dynamics);
        let inner_lit = state.sweep_factor(0.0);
        let inner_shadow = state.sweep_factor(0.5);
        assert!(inner_lit > inner_shadow);

        // Half a period later the crest is halfway along the half.
        state.advance(start + Duration::from_millis(1_500), &dynamics);
        let middle_lit = state.sweep_factor(0.5);
        assert!(
            middle_lit > state.sweep_factor(0.0),
            "the crest must have left the inner edge"
        );
        assert!(
            state.sweep_factor(1.0) < MIN_BRIGHTNESS * 1.5,
            "the shadow must stay clamped near the floor"
        );
    }

    #[test]
    fn breathe_follows_the_bass_level_not_a_clock() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let mut state = MotionState::new(settings(Motion::Breathe, 1.0, 1.0), 1);
        state.advance(start, &dynamics);
        let dark = state.breathe_factor(0.0);
        let bright = state.breathe_factor(1.0);
        assert!((dark - 0.65).abs() < 1e-6);
        assert!((bright - 1.0).abs() < 1e-6);

        // A free-running clock must not change the result.
        state.advance(start + Duration::from_secs(2), &dynamics);
        assert!((state.breathe_factor(0.5) - 0.825).abs() < 1e-6);
    }

    #[test]
    fn sparkle_only_spawns_on_a_beat_and_decays() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 1.0, 1.0), 7);
        state.advance(start, &dynamics);
        assert_eq!(active_glints(&state, 0), 0, "no beat, no glints");

        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        assert!(dynamics.is_flashing(0));
        let beat_at = start + Duration::from_millis(33);
        state.advance(beat_at, &dynamics);
        assert!(active_glints(&state, 0) > 0, "a beat must spawn glints");

        // The flash stays true; that must not re-spawn. Two hundred ms later the
        // original glints have expired.
        let later = beat_at + Duration::from_millis(200);
        state.advance(later, &dynamics);
        assert_eq!(active_glints(&state, 0), 0, "glints must decay");
    }

    #[test]
    fn sparkle_count_scales_with_intensity() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();

        let mut low = MotionState::new(settings(Motion::Sparkle, 0.25, 1.0), 7);
        let mut high = MotionState::new(settings(Motion::Sparkle, 2.0, 1.0), 7);
        low.advance(start, &dynamics);
        high.advance(start, &dynamics);
        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        let beat_at = start + Duration::from_millis(33);
        low.advance(beat_at, &dynamics);
        high.advance(beat_at, &dynamics);
        assert!(
            active_glints(&high, 0) >= active_glints(&low, 0),
            "more intensity must not spawn fewer glints"
        );
        assert!(active_glints(&high, 0) > 1);
    }

    #[test]
    fn ripple_only_starts_on_a_beat_and_expands_outward() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Ripple, 1.0, 1.0), 9);
        state.advance(start, &dynamics);
        assert_eq!(active_ripples(&state, 0), 0, "no beat, no ripple");

        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        let beat_at = start + Duration::from_millis(33);
        state.advance(beat_at, &dynamics);
        assert_eq!(active_ripples(&state, 0), 1);
        let inner_start = state.ripple_blend(0, 0);
        assert!(inner_start > 0.0);

        // Half of the 600 ms life: the ring has expanded to the middle.
        state.advance(beat_at + Duration::from_millis(300), &dynamics);
        let inner_now = state.ripple_blend(0, 0);
        let middle_now = state.ripple_blend(0, 3);
        assert!(
            middle_now > inner_now,
            "the ring should be at the middle after half its life"
        );
    }

    #[test]
    fn orbit_is_periodic_and_mirrored() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let mut state = MotionState::new(settings(Motion::Orbit, 1.0, 1.0), 1);
        state.advance(start, &dynamics);
        let cell = Cell::from_index(10).unwrap();
        let first = state.orbit_blend(cell);

        state.advance(start + Duration::from_secs(4), &dynamics);
        let repeated = state.orbit_blend(cell);
        assert!((first - repeated).abs() < 1e-5, "orbit must be periodic");
    }

    #[test]
    fn orbit_modulates_the_two_halves_as_mirror_images() {
        let mut bands = vec![0.0; BAND_COUNT];
        for (band, energy) in bands.iter_mut().enumerate() {
            *energy = 0.001 * (band + 1) as f32;
        }
        let dynamics = dynamics_with_bands(&bands);
        let start = Instant::now();
        let look = Look::default();
        let mut baseline = MotionState::default();
        baseline.advance(start, &dynamics);
        let baseline_frame = compose(&dynamics, 132, &look, &baseline);

        let mut state = MotionState::new(settings(Motion::Orbit, 1.0, 1.0), 3);
        state.advance(start, &dynamics);
        let frame = compose(&dynamics, 132, &look, &state);
        assert_ne!(
            frame, baseline_frame,
            "the orbit test must fail if orbit modulates nothing"
        );

        let left = layout::band_cells_for(&look.layout, Half::Left);
        let right = layout::band_cells_for(&look.layout, Half::Right);
        let mut compared = 0;
        for (cell, left_led) in left {
            let Some((_, right_led)) = right.iter().find(|(other, _)| *other == cell) else {
                continue;
            };
            assert_eq!(
                frame[left_led as usize], frame[*right_led as usize],
                "mirrored cell {cell:?} must receive the same orbit boost"
            );
            compared += 1;
        }
        assert!(
            compared >= 25,
            "the mirror comparison covered too few cells"
        );
    }

    #[test]
    fn rain_is_deterministic_under_a_seed() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let mut first = MotionState::new(settings(Motion::Rain, 1.0, 1.0), 42);
        let mut again = MotionState::new(settings(Motion::Rain, 1.0, 1.0), 42);
        let mut other = MotionState::new(settings(Motion::Rain, 1.0, 1.0), 43);

        first.advance(start, &dynamics);
        again.advance(start, &dynamics);
        other.advance(start, &dynamics);
        let later = start + Duration::from_millis(500);
        first.advance(later, &dynamics);
        again.advance(later, &dynamics);
        other.advance(later, &dynamics);

        assert_eq!(first.rain[0].drops, again.rain[0].drops);
        assert_ne!(first.rain[0].drops, other.rain[0].drops);
        for index in 0..layout::CELL_COUNT {
            let cell = Cell::from_index(index).unwrap();
            assert_eq!(
                first.rain_blend(0, cell),
                again.rain_blend(0, cell),
                "rain must be deterministic"
            );
        }
    }

    #[test]
    fn reset_clears_events_but_not_the_periodic_clock() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 1.0, 1.0), 11);
        state.advance(start, &dynamics);
        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        state.advance(start + Duration::from_millis(33), &dynamics);
        assert!(active_glints(&state, 0) > 0);
        assert_eq!(active_ripples(&state, 0), 0);

        let elapsed = state.elapsed;
        state.reset_events();
        assert_eq!(state.elapsed, elapsed, "reset must not reset the clock");
        assert_eq!(active_glints(&state, 0), 0);
        assert_eq!(active_ripples(&state, 0), 0);
        assert!(state.seen_beats.iter().all(Option::is_none));

        // A started ripple is also cleared on reset.
        let mut quiet = quiet_dynamics();
        let mut ripple = MotionState::new(settings(Motion::Ripple, 1.0, 1.0), 11);
        ripple.advance(start, &quiet);
        quiet.update(&beat_spectrum(), 1.0 / 30.0);
        ripple.advance(start + Duration::from_millis(33), &quiet);
        assert_eq!(active_ripples(&ripple, 0), 1);
        ripple.reset_events();
        assert_eq!(active_ripples(&ripple, 0), 0);
    }

    #[test]
    fn changing_motion_clears_events_but_keeps_the_clock() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 1.0, 1.0), 13);
        state.advance(start, &dynamics);
        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        state.advance(start + Duration::from_millis(33), &dynamics);
        assert!(active_glints(&state, 0) > 0);

        let elapsed = state.elapsed;
        state.set_motion(Motion::Ripple);
        assert_eq!(state.settings().motion, Motion::Ripple);
        assert_eq!(active_glints(&state, 0), 0);
        assert_eq!(state.elapsed, elapsed);

        state.set_settings(settings(Motion::Orbit, 1.5, 2.0));
        assert_eq!(state.settings(), settings(Motion::Orbit, 1.5, 2.0));
        assert_eq!(state.elapsed, elapsed);
    }

    #[test]
    fn modulation_clamps_at_white() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let white = Rgb::new(255, 255, 255);
        let cell = Cell::from_index(10).unwrap();

        for motion in [Motion::Sparkle, Motion::Ripple, Motion::Orbit, Motion::Rain] {
            let mut state = MotionState::new(settings(motion, 2.0, 1.0), 1);
            state.advance(start, &dynamics);
            let out = state.modulate(white, cell, Half::Left, &dynamics);
            assert_eq!(out, white, "{motion} must not overflow white");
        }
        assert_eq!(
            scale(Rgb::new(100, 200, 255), 2.0),
            Rgb::new(200, 255, 255),
            "scaling must clamp every channel to 255"
        );
    }

    #[test]
    fn motion_never_touches_the_thumbs() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let look = Look::default();

        // Sync every state before the beat so event motions actually have events.
        let mut baseline = MotionState::default();
        baseline.advance(start, &dynamics);
        let mut states: Vec<_> = Motion::ALL
            .into_iter()
            .map(|motion| (motion, MotionState::new(settings(motion, 1.5, 1.0), 5)))
            .collect();
        for (_, state) in &mut states {
            state.advance(start, &dynamics);
        }

        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        let beat_at = start + Duration::from_millis(33);
        baseline.advance(beat_at, &dynamics);
        let expected = compose(&dynamics, 132, &look, &baseline);

        for (motion, state) in &mut states {
            state.advance(beat_at, &dynamics);
            let frame = compose(&dynamics, 132, &look, state);
            for half in [Half::Left, Half::Right] {
                for led in layout::thumb_leds(half) {
                    assert_eq!(
                        frame[*led as usize], expected[*led as usize],
                        "{motion} touched a thumb"
                    );
                }
            }
        }
    }

    #[test]
    fn event_motions_see_a_beat_at_low_frame_rate() {
        // At the 5 fps floor the flash timer expires inside the same 0.2 s frame,
        // and it is already gone at 0.1 s; `is_flashing` is false by the time
        // motion advances in both cases.
        for frame_ms in [100u64, 200] {
            let start = Instant::now();
            let mut dynamics = quiet_dynamics();
            let mut state = MotionState::new(settings(Motion::Sparkle, 1.0, 1.0), 21);
            state.advance(start, &dynamics);

            dynamics.update(&beat_spectrum(), frame_ms as f32 / 1_000.0);
            assert!(
                !dynamics.is_flashing(0),
                "the flash should have expired within a {frame_ms} ms frame"
            );
            assert_eq!(dynamics.beat_count(0), 1);

            state.advance(start + Duration::from_millis(frame_ms), &dynamics);
            assert!(
                active_glints(&state, 0) > 0,
                "the beat counter must spawn glints even when the flash expired                  within a {frame_ms} ms frame"
            );
        }
    }

    #[test]
    fn overlapping_beats_each_spawn_an_event() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Ripple, 1.0, 1.0), 22);
        state.advance(start, &dynamics);

        // Two beat cycles inside one flash window. Ten-millisecond steps keep the
        // first flash alive across the second beat, so this fails if event gating
        // ever goes back to watching `is_flashing` edges.
        for beat in 0..2 {
            dynamics.update(&spectrum_with(&[0.0; BAND_COUNT]), 0.01);
            dynamics.update(&beat_spectrum(), 0.01);
            assert_eq!(dynamics.beat_count(0), beat + 1);
            assert!(
                dynamics.is_flashing(0),
                "beat {} must still be inside the flash window",
                beat + 1
            );
        }
        assert_eq!(dynamics.beat_count(0), 2);

        state.advance(start + Duration::from_millis(100), &dynamics);
        assert_eq!(
            active_ripples(&state, 0),
            2,
            "each beat must spawn its own ripple"
        );
    }

    #[test]
    fn a_beat_before_reset_does_not_fire_late_and_the_next_beat_does() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 2.0, 1.0), 23);
        state.advance(start, &dynamics);
        dynamics.update(&beat_spectrum(), 0.05);
        state.advance(start + Duration::from_millis(50), &dynamics);
        assert!(active_glints(&state, 0) > 0);

        state.reset_events_at(&dynamics);
        assert_eq!(active_glints(&state, 0), 0);

        // The count was snapshotted at reset, so the beat that was already there
        // cannot fire late on the next advance.
        state.advance(start + Duration::from_millis(100), &dynamics);
        assert_eq!(
            active_glints(&state, 0),
            0,
            "a beat from before the switch must not fire late"
        );

        // The next real beat after the switch does fire.
        dynamics.update(&spectrum_with(&[0.0; BAND_COUNT]), 0.05);
        dynamics.update(&beat_spectrum(), 0.05);
        state.advance(start + Duration::from_millis(150), &dynamics);
        assert!(
            active_glints(&state, 0) > 0,
            "the next beat after reset must spawn"
        );
    }

    #[test]
    fn a_beat_after_reset_at_and_before_advance_still_spawns() {
        // Exact switch order: beat -> advance -> reset_events_at -> beat -> advance.
        // There is deliberately no intermediate advance between the reset and the
        // post-switch beat.
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 2.0, 1.0), 23);
        state.advance(start, &dynamics);

        dynamics.update(&beat_spectrum(), 0.01);
        assert_eq!(dynamics.beat_count(0), 1);
        state.advance(start + Duration::from_millis(10), &dynamics);
        assert!(active_glints(&state, 0) > 0);

        state.reset_events_at(&dynamics);
        assert_eq!(active_glints(&state, 0), 0);

        // No advance here: the beat lands after the reset but before the frame.
        dynamics.update(&spectrum_with(&[0.0; BAND_COUNT]), 0.01);
        dynamics.update(&beat_spectrum(), 0.01);
        assert_eq!(dynamics.beat_count(0), 2);
        state.advance(start + Duration::from_millis(20), &dynamics);
        assert!(
            active_glints(&state, 0) > 0,
            "a beat after reset_events_at must not be swallowed by the next advance"
        );
    }

    #[test]
    fn motion_switch_at_snapshots_the_beat_count() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 1.0, 1.0), 26);
        state.advance(start, &dynamics);
        dynamics.update(&beat_spectrum(), 0.01); // count 1
        state.advance(start + Duration::from_millis(10), &dynamics);

        // Switch to ripple without an intermediate advance, then beat.
        state.set_motion_at(Motion::Ripple, &dynamics);
        dynamics.update(&spectrum_with(&[0.0; BAND_COUNT]), 0.01);
        dynamics.update(&beat_spectrum(), 0.01); // count 2
        state.advance(start + Duration::from_millis(20), &dynamics);
        assert_eq!(
            active_ripples(&state, 0),
            1,
            "a switch to ripple must not swallow the next beat"
        );

        // And the same through the settings variant, switching back to sparkle.
        state.set_settings_at(settings(Motion::Sparkle, 1.0, 1.0), &dynamics);
        dynamics.update(&spectrum_with(&[0.0; BAND_COUNT]), 0.01);
        dynamics.update(&beat_spectrum(), 0.01); // count 3
        state.advance(start + Duration::from_millis(30), &dynamics);
        assert!(
            active_glints(&state, 0) > 0,
            "set_settings_at must snapshot for the next beat too"
        );
    }

    #[test]
    fn mono_capture_spawns_on_both_halves() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut sparkle = MotionState::new(settings(Motion::Sparkle, 1.0, 1.0), 24);
        let mut ripple = MotionState::new(settings(Motion::Ripple, 1.0, 1.0), 24);
        sparkle.advance(start, &dynamics);
        ripple.advance(start, &dynamics);

        dynamics.update(&beat_spectrum(), 1.0 / 30.0);
        let beat_at = start + Duration::from_millis(33);
        sparkle.advance(beat_at, &dynamics);
        ripple.advance(beat_at, &dynamics);

        for half in 0..HALVES {
            assert!(
                active_glints(&sparkle, half) > 0,
                "mono sparkle must reach half {half}"
            );
            assert_eq!(
                active_ripples(&ripple, half),
                1,
                "mono ripple must reach half {half}"
            );
        }
    }

    #[test]
    fn ripple_lifetime_is_fixed_against_speed() {
        for speed in [0.25, 4.0] {
            let start = Instant::now();
            let mut dynamics = quiet_dynamics();
            let mut state = MotionState::new(settings(Motion::Ripple, 1.0, speed), 25);
            state.advance(start, &dynamics);
            dynamics.update(&beat_spectrum(), 1.0 / 30.0);
            let beat_at = start + Duration::from_millis(33);
            state.advance(beat_at, &dynamics);
            assert_eq!(active_ripples(&state, 0), 1);

            state.advance(beat_at + Duration::from_millis(599), &dynamics);
            assert_eq!(
                active_ripples(&state, 0),
                1,
                "ripple must ignore speed at 599 ms"
            );
            state.advance(beat_at + Duration::from_millis(601), &dynamics);
            assert_eq!(
                active_ripples(&state, 0),
                0,
                "ripple must end at 600 ms despite speed"
            );
        }
    }

    #[test]
    fn resets_do_not_replay_the_same_rain_pattern() {
        let start = Instant::now();
        let dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Rain, 1.0, 1.0), 42);
        state.advance(start, &dynamics);
        let first = state.rain[0].drops;
        state.reset_events();
        let second = state.rain[0].drops;
        assert_ne!(first, second, "a reset must draw a new rain pattern");

        // The same seed and reset sequence still reproduce exactly.
        let mut a = MotionState::new(settings(Motion::Rain, 1.0, 1.0), 42);
        let mut b = MotionState::new(settings(Motion::Rain, 1.0, 1.0), 42);
        a.advance(start, &dynamics);
        b.advance(start, &dynamics);
        a.reset_events();
        b.reset_events();
        a.reset_events();
        b.reset_events();
        assert_eq!(a.rain[0].drops, b.rain[0].drops);
        assert_eq!(a.rain[1].drops, b.rain[1].drops);
    }

    #[test]
    fn resets_do_not_replay_the_same_sparkle_pattern() {
        let start = Instant::now();
        let mut dynamics = quiet_dynamics();
        let mut state = MotionState::new(settings(Motion::Sparkle, 2.0, 1.0), 43);
        state.advance(start, &dynamics);
        dynamics.update(&beat_spectrum(), 0.05);
        state.advance(start + Duration::from_millis(50), &dynamics);
        let first = state.glints[0];

        state.reset_events();
        state.advance(start + Duration::from_millis(100), &dynamics); // sync
        dynamics.update(&spectrum_with(&[0.0; BAND_COUNT]), 0.05);
        dynamics.update(&beat_spectrum(), 0.05);
        state.advance(start + Duration::from_millis(150), &dynamics);
        let second = state.glints[0];
        assert_ne!(first, second, "a reset must draw new sparkle keys");

        // Identical seeds and sequences still match.
        let mut d = quiet_dynamics();
        let mut a = MotionState::new(settings(Motion::Sparkle, 2.0, 1.0), 43);
        let mut b = MotionState::new(settings(Motion::Sparkle, 2.0, 1.0), 43);
        a.advance(start, &d);
        b.advance(start, &d);
        a.reset_events();
        b.reset_events();
        a.advance(start + Duration::from_millis(50), &d);
        b.advance(start + Duration::from_millis(50), &d);
        d.update(&beat_spectrum(), 0.05);
        a.advance(start + Duration::from_millis(100), &d);
        b.advance(start + Duration::from_millis(100), &d);
        assert_eq!(a.glints[0], b.glints[0]);
    }

    #[test]
    fn sweep_and_orbit_periods_scale_with_speed() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let cell = Cell::from_index(10).unwrap();

        for speed in [0.5, 2.0] {
            let mut sweep = MotionState::new(settings(Motion::Sweep, 1.0, speed), 1);
            sweep.advance(start, &dynamics);
            let initial = sweep.sweep_factor(0.25);
            sweep.advance(start + Duration::from_secs_f32(3.0 / speed), &dynamics);
            assert!(
                (sweep.sweep_factor(0.25) - initial).abs() < 1e-5,
                "sweep period must be exactly 3 s / speed at speed {speed}"
            );

            let mut orbit = MotionState::new(settings(Motion::Orbit, 1.0, speed), 1);
            orbit.advance(start, &dynamics);
            let initial = orbit.orbit_blend(cell);
            orbit.advance(start + Duration::from_secs_f32(4.0 / speed), &dynamics);
            assert!(
                (orbit.orbit_blend(cell) - initial).abs() < 1e-5,
                "orbit period must be exactly 4 s / speed at speed {speed}"
            );
        }
    }

    #[test]
    fn rain_speed_changes_the_fall_rate() {
        let dynamics = quiet_dynamics();
        let start = Instant::now();
        let mut slow = MotionState::new(settings(Motion::Rain, 1.0, 0.5), 42);
        let mut fast = MotionState::new(settings(Motion::Rain, 1.0, 2.0), 42);
        slow.advance(start, &dynamics);
        fast.advance(start, &dynamics);
        let later = start + Duration::from_millis(500);
        slow.advance(later, &dynamics);
        fast.advance(later, &dynamics);

        let differs = (0..layout::CELL_COUNT).any(|index| {
            let cell = Cell::from_index(index).unwrap();
            (slow.rain_blend(0, cell) - fast.rain_blend(0, cell)).abs() > 1e-4
        });
        assert!(differs, "speed must change how far the rain has fallen");
    }

    #[test]
    fn breathe_follows_the_thumb_level_through_compose() {
        let start = Instant::now();
        let quiet = quiet_dynamics();
        let mut loud_bands = vec![0.8; BAND_COUNT];
        loud_bands[..BASS_BANDS].fill(0.8);
        let loud = dynamics_with_bands(&loud_bands);
        let look = Look::default();
        let mut state = MotionState::new(settings(Motion::Breathe, 1.0, 1.0), 1);
        state.advance(start, &quiet);

        // Same clock, same state: only the thumb level differs.
        let quiet_frame = compose(&quiet, 132, &look, &state);
        let loud_frame = compose(&loud, 132, &look, &state);
        let led = layout::band_leds(Half::Left)[0] as usize;
        let brightness = |colour: Rgb| colour.r as u32 + colour.g as u32 + colour.b as u32;
        assert!(
            brightness(loud_frame[led]) > brightness(quiet_frame[led]),
            "breathe must follow the thumb level through compose"
        );
    }
}
