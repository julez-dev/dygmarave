//! The look selection engine: three independent axes, shuffle bags, jittered
//! timers, and a crossfade from the colours actually on screen.
//!
//! The engine owns two explicit RNG streams derived from one seed:
//!
//! * the selection stream, seeded with the look seed, drives shuffle-bag draws,
//!   scatter permutations and timer jitter;
//! * the motion stream, seeded with `seed ^ MOTION_SEED_SALT`, drives motion event
//!   randomness such as sparkle keys and rain droplets.
//!
//! Keeping them separate means selection draws cannot perturb the motion show,
//! while the same look seed still reproduces both. Motion event state lives in
//! [`MotionState`], which is advanced exactly once per frame by
//! [`LookState::render`]; switches reset its events with the current [`Dynamics`]
//! before that advance.

use std::fmt;
use std::time::{Duration, Instant};

use crate::dynamics::Dynamics;
use crate::layout::{self, Cell, Half, Layout, LayoutKind};
use crate::motion::{Motion, MotionSettings, MotionState};
use crate::palette::Palette;
use crate::render;
use crate::theme::Rgb;

/// Salt that separates the motion-event RNG stream from the selection stream.
const MOTION_SEED_SALT: u64 = 0x9e37_79b9_7f4a_7c15;

/// One of the three look axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Layout,
    Palette,
    Motion,
}

impl Axis {
    pub const fn name(self) -> &'static str {
        match self {
            Axis::Layout => "layout",
            Axis::Palette => "palette",
            Axis::Motion => "motion",
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why an axis switched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    /// A random axis's deadline elapsed and drew from its shuffle bag.
    Random,
}

impl SwitchReason {
    pub const fn name(self) -> &'static str {
        match self {
            SwitchReason::Random => "random",
        }
    }
}

impl fmt::Display for SwitchReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One axis change, ready for ticket 04's switch log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub axis: Axis,
    pub from: &'static str,
    pub to: &'static str,
    pub reason: SwitchReason,
}

/// How one axis is selected.
#[derive(Debug, Clone, PartialEq)]
pub enum AxisSettings<T> {
    /// A fixed value. It never draws and never schedules.
    Fixed(T),
    /// A shuffle bag over `pool`, redrawn on a jittered `interval`.
    ///
    /// An interval of zero draws once at startup and never switches.
    #[cfg_attr(not(test), allow(dead_code))]
    Random { pool: Vec<T>, interval: Duration },
}

impl<T> AxisSettings<T> {
    pub fn fixed(value: T) -> Self {
        AxisSettings::Fixed(value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn random(pool: Vec<T>, interval: Duration) -> Self {
        AxisSettings::Random { pool, interval }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_random(&self) -> bool {
        matches!(self, AxisSettings::Random { .. })
    }
}

/// A shuffle bag: draws each pool item once per cycle, reshuffling only when
/// exhausted, and never draws the same value across the boundary.
#[derive(Debug, Clone)]
pub struct Bag<T> {
    items: Vec<T>,
    remaining: Vec<T>,
    last: Option<T>,
}

impl<T: Clone + PartialEq> Bag<T> {
    pub fn new(items: Vec<T>) -> Self {
        assert!(!items.is_empty(), "a random axis pool must not be empty");
        Self {
            remaining: items.clone(),
            items,
            last: None,
        }
    }

    /// Draws without replacement. One-element pools return the element without
    /// consuming RNG or reshuffling.
    pub fn draw(&mut self, rng: &mut fastrand::Rng) -> T {
        if self.items.len() == 1 {
            return self.items[0].clone();
        }
        if self.last.is_none() || self.remaining.is_empty() {
            if self.remaining.is_empty() {
                self.remaining.clone_from(&self.items);
            }
            shuffle(&mut self.remaining, rng);
            if self.last.is_some() && self.remaining.last() == self.last.as_ref() {
                let last = self.remaining.len() - 1;
                self.remaining.swap(last, 0);
            }
        }
        let value = self
            .remaining
            .pop()
            .expect("a non-empty bag must have a remaining item");
        self.last = Some(value.clone());
        value
    }
}

fn shuffle<T>(items: &mut [T], rng: &mut fastrand::Rng) {
    for index in (1..items.len()).rev() {
        let swap = rng.usize(..=index);
        items.swap(index, swap);
    }
}

/// The three selected names: layout kind, palette and motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub layout: LayoutKind,
    pub palette: Palette,
    pub motion: Motion,
}

/// A selection with its layout resolved and its band table cached.
///
/// The band table is computed once when the selection is resolved, never per
/// frame, so a crossfade does not rerun a layout permutation.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLook {
    selection: Selection,
    layout: Layout,
    bands: [Vec<(Cell, u8)>; 2],
}

impl ResolvedLook {
    pub fn new(selection: Selection, layout: Layout) -> Self {
        let bands = [
            layout::band_cells_for(&layout, Half::Left),
            layout::band_cells_for(&layout, Half::Right),
        ];
        Self {
            selection,
            layout,
            bands,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selection(&self) -> Selection {
        self.selection
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn palette(&self) -> Palette {
        self.selection.palette
    }

    pub(crate) fn bands(&self, half: Half) -> &[(Cell, u8)] {
        let index = match half {
            Half::Left => 0,
            Half::Right => 1,
        };
        &self.bands[index]
    }

    fn set_selection(&mut self, selection: Selection) {
        self.selection = selection;
    }
}

/// Engine configuration. The default is the v0.1 look.
#[derive(Debug, Clone, PartialEq)]
pub struct LookSettings {
    pub layout: AxisSettings<LayoutKind>,
    pub palette: AxisSettings<Palette>,
    pub motion: AxisSettings<Motion>,
    pub fade_ms: u32,
    pub motion_intensity: f32,
    pub motion_speed: f32,
}

impl Default for LookSettings {
    fn default() -> Self {
        Self {
            layout: AxisSettings::fixed(LayoutKind::Snake),
            palette: AxisSettings::fixed(Palette::Ember),
            motion: AxisSettings::fixed(Motion::None),
            fade_ms: 300,
            motion_intensity: 1.0,
            motion_speed: 1.0,
        }
    }
}

impl LookSettings {
    fn motion_settings(&self, motion: Motion) -> MotionSettings {
        MotionSettings {
            motion,
            intensity: self.motion_intensity,
            speed: self.motion_speed,
        }
    }
}

#[derive(Debug, Clone)]
struct AxisRunner<T> {
    settings: AxisSettings<T>,
    bag: Option<Bag<T>>,
    deadline: Option<Instant>,
}

impl<T: Clone + PartialEq> AxisRunner<T> {
    fn new(settings: AxisSettings<T>) -> Self {
        let bag = match &settings {
            AxisSettings::Fixed(_) => None,
            AxisSettings::Random { pool, .. } => Some(Bag::new(pool.clone())),
        };
        Self {
            settings,
            bag,
            deadline: None,
        }
    }

    fn startup(&mut self, rng: &mut fastrand::Rng, now: Instant) -> T {
        let value = self.draw(rng);
        self.schedule(rng, now);
        value
    }

    fn poll(&mut self, now: Instant, rng: &mut fastrand::Rng) -> Option<T> {
        let due = self.deadline.is_some_and(|deadline| now >= deadline);
        if !due {
            return None;
        }
        let value = self.draw(rng);
        self.schedule(rng, now);
        Some(value)
    }

    fn draw(&mut self, rng: &mut fastrand::Rng) -> T {
        match &self.settings {
            AxisSettings::Fixed(value) => value.clone(),
            AxisSettings::Random { .. } => self
                .bag
                .as_mut()
                .expect("a random axis must have a bag")
                .draw(rng),
        }
    }

    fn schedule(&mut self, rng: &mut fastrand::Rng, now: Instant) {
        let interval = match &self.settings {
            AxisSettings::Random { interval, .. } => *interval,
            AxisSettings::Fixed(_) => {
                self.deadline = None;
                return;
            }
        };
        if interval.is_zero() {
            self.deadline = None;
        } else {
            self.deadline = Some(now + jitter(interval, rng));
        }
    }
}

/// Uniform ±25% jitter around an interval.
fn jitter(interval: Duration, rng: &mut fastrand::Rng) -> Duration {
    let factor = 0.75 + 0.5 * rng.f64();
    Duration::from_secs_f64(interval.as_secs_f64() * factor)
}

/// The run-time engine: current selection, timers, motion state and fade.
pub struct LookState {
    settings: LookSettings,
    rng: fastrand::Rng,
    layout: AxisRunner<LayoutKind>,
    palette: AxisRunner<Palette>,
    motion: AxisRunner<Motion>,
    current: ResolvedLook,
    motion_state: MotionState,
    last_displayed: Vec<Rgb>,
    fade_from: Option<Vec<Rgb>>,
    fade_started: Option<Instant>,
}

impl LookState {
    /// Resolves the startup selections before the first frame.
    ///
    /// Random axes draw once here; a fixed `Scatter` resolves its one permutation
    /// here too. No fade starts at startup.
    pub fn new(settings: LookSettings, seed: u64, now: Instant) -> Self {
        let mut rng = fastrand::Rng::with_seed(seed);
        let mut layout = AxisRunner::new(settings.layout.clone());
        let mut palette = AxisRunner::new(settings.palette.clone());
        let mut motion = AxisRunner::new(settings.motion.clone());

        let layout_kind = layout.startup(&mut rng, now);
        let palette_value = palette.startup(&mut rng, now);
        let motion_value = motion.startup(&mut rng, now);
        let resolved_layout = layout_kind.resolve(&mut rng);
        let selection = Selection {
            layout: layout_kind,
            palette: palette_value,
            motion: motion_value,
        };
        let motion_state = MotionState::new(
            settings.motion_settings(motion_value),
            seed ^ MOTION_SEED_SALT,
        );

        Self {
            settings,
            rng,
            layout,
            palette,
            motion,
            current: ResolvedLook::new(selection, resolved_layout),
            motion_state,
            last_displayed: Vec::new(),
            fade_from: None,
            fade_started: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settings(&self) -> &LookSettings {
        &self.settings
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selection(&self) -> Selection {
        self.current.selection
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn resolved(&self) -> &ResolvedLook {
        &self.current
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_displayed(&self) -> &[Rgb] {
        &self.last_displayed
    }

    /// Fires every axis whose deadline has passed. Multiple axes may switch on the
    /// same tick; all of them are returned.
    ///
    /// A draw that resolves to the current value is a no-op: its timer still
    /// advances, but it emits no [`Switch`] and starts no fade.
    pub fn tick(&mut self, now: Instant, dynamics: &Dynamics) -> Vec<Switch> {
        let previous = self.current.selection;
        let mut next = previous;
        let mut new_layout = None;
        let mut switches = Vec::new();

        if let Some(kind) = self.layout.poll(now, &mut self.rng) {
            // Scatter always resolves a fresh permutation, even when the draw is
            // the same kind; that is the axis's whole point.
            let needs_resolution = kind != previous.layout || kind == LayoutKind::Scatter;
            if needs_resolution {
                let layout = kind.resolve(&mut self.rng);
                if layout != self.current.layout {
                    next.layout = kind;
                    new_layout = Some(layout);
                    switches.push(Switch {
                        axis: Axis::Layout,
                        from: previous.layout.name(),
                        to: kind.name(),
                        reason: SwitchReason::Random,
                    });
                }
            }
        }

        if let Some(palette) = self.palette.poll(now, &mut self.rng) {
            if palette != previous.palette {
                next.palette = palette;
                switches.push(Switch {
                    axis: Axis::Palette,
                    from: previous.palette.name(),
                    to: palette.name(),
                    reason: SwitchReason::Random,
                });
            }
        }

        if let Some(motion) = self.motion.poll(now, &mut self.rng) {
            if motion != previous.motion {
                next.motion = motion;
                switches.push(Switch {
                    axis: Axis::Motion,
                    from: previous.motion.name(),
                    to: motion.name(),
                    reason: SwitchReason::Random,
                });
            }
        }

        if switches.is_empty() {
            return switches;
        }

        if let Some(layout) = new_layout {
            self.current = ResolvedLook::new(next, layout);
        } else {
            self.current.set_selection(next);
        }

        if !self.last_displayed.is_empty() && self.settings.fade_ms > 0 {
            self.fade_from = Some(self.last_displayed.clone());
            self.fade_started = Some(now);
        } else {
            self.fade_from = None;
            self.fade_started = None;
        }

        // Do this before the frame's motion advance; `_at` snapshots the current
        // beat counters so a beat after the switch cannot be swallowed.
        let motion_settings = self.settings.motion_settings(next.motion);
        self.motion_state.set_settings_at(motion_settings, dynamics);

        switches
    }

    /// Smoothstep alpha in `0..=1`, reaching 1 exactly at the fade deadline.
    pub fn fade_alpha(&self, now: Instant) -> f32 {
        if self.fade_from.is_none() {
            return 1.0;
        }
        let Some(started) = self.fade_started else {
            return 1.0;
        };
        if self.settings.fade_ms == 0 {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= Duration::from_millis(self.settings.fade_ms as u64) {
            return 1.0;
        }
        let t = (elapsed.as_secs_f64() * 1_000.0 / self.settings.fade_ms as f64).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t)) as f32
    }

    /// Advances motion once, renders the current selection, and blends it from the
    /// screen snapshot if a fade is active.
    pub fn render(&mut self, now: Instant, dynamics: &Dynamics, count: usize) -> Vec<Rgb> {
        self.motion_state.advance(now, dynamics);
        let mut frame = render::compose_bands(
            dynamics,
            count,
            self.current.palette(),
            self.current.bands(Half::Left),
            self.current.bands(Half::Right),
            &self.motion_state,
        );

        let alpha = self.fade_alpha(now);
        let fading = self
            .fade_from
            .as_ref()
            .is_some_and(|from| blend_into(&mut frame, from, alpha));
        if !fading || alpha >= 1.0 {
            self.fade_from = None;
            self.fade_started = None;
        }

        // Reuse the previous frame's allocation as the return buffer when one
        // exists. Compose already allocates a Vec; this keeps the fade engine to
        // that one allocation after the first frame.
        let mut out = std::mem::replace(&mut self.last_displayed, frame);
        out.clone_from(&self.last_displayed);
        out
    }
}

/// The engine's one colour blend. Returns false (a cut) on a length mismatch.
fn blend_into(target: &mut [Rgb], from: &[Rgb], alpha: f32) -> bool {
    if target.len() != from.len() {
        return false;
    }
    for (out, from) in target.iter_mut().zip(from) {
        *out = from.lerp(*out, alpha);
    }
    true
}

#[cfg(test)]
fn blend(from: &[Rgb], to: &[Rgb], alpha: f32) -> Vec<Rgb> {
    let mut out = to.to_vec();
    blend_into(&mut out, from, alpha);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::{Spectrum, BAND_COUNT};

    const LEDS: usize = 132;

    fn dynamics_with_bands(values: &[f32]) -> Dynamics {
        let mut spectrum = Spectrum::new(1, values.len());
        spectrum.set_bands(0, values);
        let mut dynamics = Dynamics::new(crate::dynamics::DynamicsConfig::default(), 1, BAND_COUNT);
        dynamics.update(&spectrum, 1.0 / 30.0);
        dynamics
    }

    fn varied_dynamics() -> Dynamics {
        let mut bands = vec![0.0; BAND_COUNT];
        for (band, energy) in bands.iter_mut().enumerate() {
            *energy = 0.001 * (band + 1) as f32;
        }
        dynamics_with_bands(&bands)
    }

    fn target_frame(look: &LookState, dynamics: &Dynamics) -> Vec<Rgb> {
        render::compose_bands(
            dynamics,
            LEDS,
            look.resolved().palette(),
            look.resolved().bands(Half::Left),
            look.resolved().bands(Half::Right),
            &look.motion_state,
        )
    }

    fn force_next<T: Clone + PartialEq>(runner: &mut AxisRunner<T>, value: T, at: Instant) {
        let bag = runner.bag.as_mut().expect("test needs a random axis");
        bag.remaining = vec![value];
        bag.last = None;
        runner.deadline = Some(at);
    }

    fn force_deadline<T>(runner: &mut AxisRunner<T>, at: Instant) {
        runner.deadline = Some(at);
    }

    fn other<T: Clone + PartialEq>(items: &[T], current: T) -> T {
        items
            .iter()
            .find(|item| **item != current)
            .expect("the pool needs a value other than the current one")
            .clone()
    }

    fn random_palette_settings(fade_ms: u32) -> LookSettings {
        LookSettings {
            palette: AxisSettings::random(
                vec![Palette::Ember, Palette::Ice, Palette::Solar],
                Duration::from_secs(10),
            ),
            fade_ms,
            ..LookSettings::default()
        }
    }

    #[test]
    fn a_bag_covers_the_pool_before_repeating() {
        let mut bag = Bag::new(vec![1, 2, 3, 4]);
        let mut rng = fastrand::Rng::with_seed(7);
        let mut cycle: Vec<_> = (0..4).map(|_| bag.draw(&mut rng)).collect();
        cycle.sort_unstable();
        assert_eq!(cycle, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_bag_never_repeats_across_a_boundary() {
        let mut bag = Bag::new(vec![1, 2, 3]);
        let mut rng = fastrand::Rng::with_seed(11);
        let mut previous = None;
        for _ in 0..100 {
            let value = bag.draw(&mut rng);
            assert_ne!(Some(value), previous, "boundary repeated {value}");
            previous = Some(value);
        }
    }

    #[test]
    fn a_bags_first_draw_is_seeded() {
        let mut bag = Bag::new(vec![1, 2, 3, 4]);
        assert_eq!(bag.draw(&mut fastrand::Rng::with_seed(7)), 4);

        let mut firsts: Vec<_> = (0..32)
            .map(|seed| {
                let mut bag = Bag::new(vec![1, 2, 3, 4]);
                bag.draw(&mut fastrand::Rng::with_seed(seed))
            })
            .collect();
        firsts.sort_unstable();
        firsts.dedup();
        assert!(
            firsts.len() > 1,
            "the first draw must depend on the seed, got {firsts:?}"
        );
    }

    #[test]
    fn a_one_element_bag_neither_reshuffles_nor_consumes_rng() {
        let mut bag = Bag::new(vec![42]);
        let mut used = fastrand::Rng::with_seed(3);
        let mut untouched = fastrand::Rng::with_seed(3);
        for _ in 0..5 {
            assert_eq!(bag.draw(&mut used), 42);
        }
        assert_eq!(used.f64(), untouched.f64());
    }

    #[test]
    fn random_axes_draw_at_startup_and_never_before_the_first_deadline() {
        let start = Instant::now();
        let settings = LookSettings {
            palette: AxisSettings::random(
                vec![Palette::Ember, Palette::Ice],
                Duration::from_secs(10),
            ),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 21, start);
        assert!(look.settings().palette.is_random());
        assert_eq!(look.resolved().selection(), look.selection());
        assert!(matches!(
            look.selection().palette,
            Palette::Ember | Palette::Ice
        ));
        assert!(look.palette.deadline.is_some());

        let early = look.tick(start + Duration::from_secs(1), &varied_dynamics());
        assert!(early.is_empty());
    }

    #[test]
    fn a_zero_interval_draws_once_and_never_switches() {
        let start = Instant::now();
        let settings = LookSettings {
            palette: AxisSettings::random(vec![Palette::Ember, Palette::Ice], Duration::ZERO),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 22, start);
        let selected = look.selection().palette;
        assert!(look.palette.deadline.is_none());
        let switches = look.tick(start + Duration::from_secs(86_400), &varied_dynamics());
        assert!(switches.is_empty());
        assert_eq!(look.selection().palette, selected);
    }

    #[test]
    fn a_fixed_scatter_resolves_once_and_never_schedules() {
        let start = Instant::now();
        let settings = LookSettings {
            layout: AxisSettings::fixed(LayoutKind::Scatter),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 23, start);
        assert_eq!(look.selection().layout, LayoutKind::Scatter);
        assert!(look.layout.deadline.is_none());
        let resolved = look.resolved().layout().clone();
        let switches = look.tick(start + Duration::from_secs(86_400), &varied_dynamics());
        assert!(switches.is_empty());
        assert_eq!(look.resolved().layout(), &resolved);
    }

    #[test]
    fn a_resolved_look_caches_the_band_table() {
        let start = Instant::now();
        let look = LookState::new(LookSettings::default(), 25, start);
        for half in [Half::Left, Half::Right] {
            assert_eq!(
                look.resolved().bands(half),
                layout::band_cells_for(look.resolved().layout(), half).as_slice(),
                "the resolved selection must cache the exact band table"
            );
        }
    }

    #[test]
    fn a_random_scatter_resolves_a_fresh_permutation_each_draw() {
        let start = Instant::now();
        let settings = LookSettings {
            layout: AxisSettings::random(vec![LayoutKind::Scatter], Duration::from_secs(10)),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 24, start);
        let first = look.resolved().layout().clone();
        force_deadline(&mut look.layout, start + Duration::from_secs(1));
        let switches = look.tick(start + Duration::from_secs(1), &varied_dynamics());
        assert_eq!(switches.len(), 1);
        assert_eq!(switches[0].axis, Axis::Layout);
        assert_eq!(switches[0].from, "scatter");
        assert_eq!(switches[0].to, "scatter");
        assert_ne!(look.resolved().layout(), &first);
    }

    #[test]
    fn jitter_is_bounded_and_reproducible_under_a_seed() {
        let start = Instant::now();
        let interval = Duration::from_secs(10);
        let mut deadlines = Vec::new();
        for seed in 0..64 {
            let settings = LookSettings {
                layout: AxisSettings::random(vec![LayoutKind::Snake, LayoutKind::Rows], interval),
                ..LookSettings::default()
            };
            let look = LookState::new(settings, seed, start);
            let deadline = look.layout.deadline.expect("a random axis schedules");
            let delta = deadline.saturating_duration_since(start).as_secs_f64();
            assert!(
                (7.5..=12.5).contains(&delta),
                "jitter out of range: {delta}"
            );
            deadlines.push(deadline);
        }

        let settings = LookSettings {
            layout: AxisSettings::random(vec![LayoutKind::Snake, LayoutKind::Rows], interval),
            ..LookSettings::default()
        };
        let a = LookState::new(settings.clone(), 1234, start);
        let b = LookState::new(settings, 1234, start);
        assert_eq!(a.layout.deadline, b.layout.deadline);
        assert_eq!(a.selection(), b.selection());
    }

    #[test]
    fn axes_keep_independent_deadlines() {
        let start = Instant::now();
        let settings = LookSettings {
            layout: AxisSettings::random(
                vec![LayoutKind::Snake, LayoutKind::Rows],
                Duration::from_secs(1_000),
            ),
            palette: AxisSettings::random(
                vec![Palette::Ember, Palette::Ice],
                Duration::from_secs(2_000),
            ),
            motion: AxisSettings::random(
                vec![Motion::None, Motion::Sweep],
                Duration::from_secs(3_000),
            ),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 30, start);
        let dynamics = varied_dynamics();
        let t1 = start + Duration::from_secs(4);
        force_deadline(&mut look.palette, t1);
        force_deadline(&mut look.layout, t1 + Duration::from_secs(1));
        force_deadline(&mut look.motion, t1 + Duration::from_secs(2));

        let first = look.tick(t1, &dynamics);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].axis, Axis::Palette);

        let second = look.tick(t1 + Duration::from_secs(1), &dynamics);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].axis, Axis::Layout);

        let third = look.tick(t1 + Duration::from_secs(2), &dynamics);
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].axis, Axis::Motion);
    }

    #[test]
    fn several_axes_can_switch_on_the_same_tick() {
        let start = Instant::now();
        let settings = LookSettings {
            layout: AxisSettings::random(
                vec![LayoutKind::Snake, LayoutKind::Rows],
                Duration::from_secs(10),
            ),
            palette: AxisSettings::random(
                vec![Palette::Ember, Palette::Ice],
                Duration::from_secs(10),
            ),
            motion: AxisSettings::random(
                vec![Motion::None, Motion::Sweep],
                Duration::from_secs(10),
            ),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 31, start);
        let dynamics = varied_dynamics();
        look.render(start, &dynamics, LEDS);

        let next_layout = other(
            &[LayoutKind::Snake, LayoutKind::Rows],
            look.selection().layout,
        );
        let next_palette = other(&[Palette::Ember, Palette::Ice], look.selection().palette);
        let next_motion = other(&[Motion::None, Motion::Sweep], look.selection().motion);
        let at = start + Duration::from_secs(1);
        force_next(&mut look.layout, next_layout, at);
        force_next(&mut look.palette, next_palette, at);
        force_next(&mut look.motion, next_motion, at);

        let switches = look.tick(at, &dynamics);
        assert_eq!(switches.len(), 3);
        assert!(switches.iter().any(|switch| switch.axis == Axis::Layout));
        assert!(switches.iter().any(|switch| switch.axis == Axis::Palette));
        assert!(switches.iter().any(|switch| switch.axis == Axis::Motion));
        assert!(switches
            .iter()
            .all(|switch| switch.reason == SwitchReason::Random));
        assert_eq!(look.selection().layout, next_layout);
        assert_eq!(look.selection().palette, next_palette);
        assert_eq!(look.selection().motion, next_motion);
        assert_eq!(look.resolved().selection(), look.selection());
        assert_eq!(
            look.resolved().layout(),
            &next_layout.resolve(&mut fastrand::Rng::with_seed(0)),
            "the resolved layout must match the drawn kind"
        );
        assert!(look.fade_from.is_some(), "one fade covers the whole tick");
    }

    #[test]
    fn a_late_tick_emits_one_switch_and_reschedules_from_now() {
        let start = Instant::now();
        let interval = Duration::from_secs(10);
        let settings = LookSettings {
            palette: AxisSettings::random(
                vec![Palette::Ember, Palette::Ice, Palette::Solar],
                interval,
            ),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 40, start);
        let dynamics = varied_dynamics();
        let late = start + Duration::from_secs(60);
        force_deadline(&mut look.palette, start + Duration::from_secs(1));

        let switches = look.tick(late, &dynamics);
        assert_eq!(switches.len(), 1, "a late tick must not burst");
        let again = look.tick(late, &dynamics);
        assert!(again.is_empty(), "the same deadline cannot fire twice");
        let next = look.palette.deadline.expect("rescheduled");
        let delta = next.saturating_duration_since(late).as_secs_f64();
        assert!(
            (7.5..=12.5).contains(&delta),
            "rescheduling must be one jittered interval from now, got {delta}"
        );
    }

    #[test]
    fn a_no_op_draw_emits_no_switch_but_still_advances_its_timer() {
        let start = Instant::now();
        let settings = LookSettings {
            palette: AxisSettings::random(vec![Palette::Ember], Duration::from_secs(1)),
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 32, start);
        let dynamics = varied_dynamics();
        look.render(start, &dynamics, LEDS);
        let before = look.palette.deadline.expect("scheduled");
        let at = start + Duration::from_secs(2);
        force_deadline(&mut look.palette, at);

        let switches = look.tick(at, &dynamics);
        assert!(switches.is_empty(), "same value must be a no-op");
        assert!(look.fade_from.is_none(), "a no-op must not fade");
        assert!(look.palette.deadline.expect("rescheduled") > before);
    }

    #[test]
    fn startup_does_not_fade_and_the_default_frame_is_v0_1() {
        let start = Instant::now();
        let dynamics = varied_dynamics();
        let mut look = LookState::new(LookSettings::default(), 33, start);
        let frame = look.render(start, &dynamics, LEDS);
        assert_eq!(look.last_displayed(), frame.as_slice());
        assert!(look.fade_from.is_none());
        let expected = render::compose(
            &dynamics,
            LEDS,
            &render::Look::default(),
            &MotionState::default(),
        );
        assert_eq!(frame, expected);
    }

    #[test]
    fn a_fade_starts_from_the_displayed_frame_and_completes_exactly() {
        let start = Instant::now();
        let dynamics = varied_dynamics();
        let settings = random_palette_settings(100);
        let mut look = LookState::new(settings, 34, start);
        let before = look.render(start, &dynamics, LEDS);
        assert!(look.fade_from.is_none(), "startup must not fade");

        let next_palette = other(
            &[Palette::Ember, Palette::Ice, Palette::Solar],
            look.selection().palette,
        );
        let switch_at = start + Duration::from_secs(1);
        force_next(&mut look.palette, next_palette, switch_at);
        let switches = look.tick(switch_at, &dynamics);
        assert_eq!(switches.len(), 1);
        assert_eq!(switches[0].axis, Axis::Palette);
        assert_eq!(look.fade_from.as_deref(), Some(before.as_slice()));

        let at_switch = look.render(switch_at, &dynamics, LEDS);
        assert_eq!(at_switch, before, "alpha zero must reproduce the old frame");

        let target = target_frame(&look, &dynamics);
        let midpoint = switch_at + Duration::from_millis(50);
        let mid = look.render(midpoint, &dynamics, LEDS);
        assert_eq!(mid, blend(&before, &target, 0.5));

        let deadline = switch_at + Duration::from_millis(100);
        let landed = look.render(deadline, &dynamics, LEDS);
        assert_eq!(landed, target, "the fade must land exactly at its deadline");
        assert!(
            look.fade_from.is_none(),
            "the snapshot is cleared at alpha 1"
        );
    }

    #[test]
    fn a_mid_fade_switch_restarts_from_the_current_blend_without_a_jump() {
        let start = Instant::now();
        let dynamics = varied_dynamics();
        let settings = random_palette_settings(100);
        let mut look = LookState::new(settings, 35, start);
        look.render(start, &dynamics, LEDS);

        let first_palette = other(
            &[Palette::Ember, Palette::Ice, Palette::Solar],
            look.selection().palette,
        );
        let first_switch = start + Duration::from_secs(1);
        force_next(&mut look.palette, first_palette, first_switch);
        look.tick(first_switch, &dynamics);
        let mid_time = first_switch + Duration::from_millis(50);
        look.render(mid_time, &dynamics, LEDS);
        let displayed = look.last_displayed.clone();

        let second_palette = other(
            &[Palette::Ember, Palette::Ice, Palette::Solar],
            look.selection().palette,
        );
        let second_switch = mid_time + Duration::from_millis(10);
        force_next(&mut look.palette, second_palette, second_switch);
        look.tick(second_switch, &dynamics);
        assert_eq!(look.fade_from.as_deref(), Some(displayed.as_slice()));

        let at_switch = look.render(second_switch, &dynamics, LEDS);
        assert_eq!(at_switch, displayed, "the retarget must not jump");

        let deadline = second_switch + Duration::from_millis(100);
        let target = target_frame(&look, &dynamics);
        let landed = look.render(deadline, &dynamics, LEDS);
        assert_eq!(landed, target);
    }

    #[test]
    fn fade_zero_cuts_immediately() {
        let start = Instant::now();
        let dynamics = varied_dynamics();
        let settings = random_palette_settings(0);
        let mut look = LookState::new(settings, 36, start);
        look.render(start, &dynamics, LEDS);
        let next_palette = other(
            &[Palette::Ember, Palette::Ice, Palette::Solar],
            look.selection().palette,
        );
        let switch_at = start + Duration::from_secs(1);
        force_next(&mut look.palette, next_palette, switch_at);
        look.tick(switch_at, &dynamics);
        assert!(look.fade_from.is_none());
        let target = target_frame(&look, &dynamics);
        assert_eq!(look.render(switch_at, &dynamics, LEDS), target);
    }

    #[test]
    fn a_motion_switch_clears_events_but_keeps_the_clock() {
        let start = Instant::now();
        let mut dynamics = dynamics_with_bands(&[0.0; BAND_COUNT]);
        let settings = LookSettings {
            motion: AxisSettings::random(
                vec![Motion::Sparkle, Motion::Ripple],
                Duration::from_secs(10),
            ),
            fade_ms: 0,
            ..LookSettings::default()
        };
        let mut look = LookState::new(settings, 37, start);
        // Force a known starting motion and matching state, synced on silence.
        look.current.set_selection(Selection {
            motion: Motion::Sparkle,
            ..look.selection()
        });
        look.motion_state.set_settings_at(
            MotionSettings {
                motion: Motion::Sparkle,
                intensity: 1.0,
                speed: 1.0,
            },
            &dynamics,
        );
        look.render(start, &dynamics, LEDS);
        assert_eq!(look.motion_state.active_event_count(), 0);

        // A real beat after that sync spawns glints.
        let mut bands = vec![0.0; BAND_COUNT];
        bands[..crate::dynamics::BASS_BANDS].fill(0.8);
        let mut spectrum = Spectrum::new(1, BAND_COUNT);
        spectrum.set_bands(0, &bands);
        dynamics.update(&spectrum, 1.0 / 30.0);
        let beat_at = start + Duration::from_millis(33);
        look.render(beat_at, &dynamics, LEDS);
        assert!(look.motion_state.active_event_count() > 0);
        let elapsed = look.motion_state.elapsed_seconds();

        let switch_at = start + Duration::from_secs(1);
        force_next(&mut look.motion, Motion::Ripple, switch_at);
        let switches = look.tick(switch_at, &dynamics);
        assert_eq!(switches.len(), 1);
        assert_eq!(switches[0].axis, Axis::Motion);
        assert_eq!(look.motion_state.active_event_count(), 0);
        assert_eq!(look.motion_state.elapsed_seconds(), elapsed);

        // And the frame after the switch advances normally without the old glints.
        look.render(switch_at, &dynamics, LEDS);
        assert_eq!(look.motion_state.active_event_count(), 0);
    }

    #[test]
    fn the_same_seed_reproduces_draws_scatter_and_motion() {
        let start = Instant::now();
        let settings = LookSettings {
            layout: AxisSettings::random(
                vec![LayoutKind::Snake, LayoutKind::Rows, LayoutKind::Scatter],
                Duration::from_secs(1),
            ),
            palette: AxisSettings::random(
                vec![Palette::Ember, Palette::Ice, Palette::Solar],
                Duration::from_secs(2),
            ),
            motion: AxisSettings::random(
                vec![Motion::None, Motion::Rain, Motion::Sparkle],
                Duration::from_secs(3),
            ),
            fade_ms: 0,
            ..LookSettings::default()
        };
        let dynamics = varied_dynamics();
        let mut a = LookState::new(settings.clone(), 99, start);
        let mut b = LookState::new(settings, 99, start);
        assert_eq!(a.selection(), b.selection());
        assert_eq!(a.resolved(), b.resolved());

        for step in 1..=20 {
            let now = start + Duration::from_millis(step * 250);
            let switches_a = a.tick(now, &dynamics);
            let switches_b = b.tick(now, &dynamics);
            assert_eq!(switches_a, switches_b);
            assert_eq!(a.selection(), b.selection());
            assert_eq!(a.resolved(), b.resolved());
            assert_eq!(
                a.render(now, &dynamics, LEDS),
                b.render(now, &dynamics, LEDS)
            );
        }
    }

    #[test]
    fn different_seeds_differ_for_interval_zero_random_axes() {
        let start = Instant::now();
        let settings = LookSettings {
            layout: AxisSettings::random(
                vec![LayoutKind::Snake, LayoutKind::Rows, LayoutKind::Scatter],
                Duration::ZERO,
            ),
            palette: AxisSettings::random(vec![Palette::Ember, Palette::Ice], Duration::ZERO),
            motion: AxisSettings::random(vec![Motion::None, Motion::Sweep], Duration::ZERO),
            ..LookSettings::default()
        };
        let a = LookState::new(settings.clone(), 1, start);
        let b = LookState::new(settings, 2, start);
        assert!(a.layout.deadline.is_none());
        assert!(b.layout.deadline.is_none());
        assert_ne!(a.selection(), b.selection());
        assert_ne!(a.resolved(), b.resolved());
    }
}
