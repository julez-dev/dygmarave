//! The physical key grid, and patterns for checking it against the real board.
//!
//! # Which map this is
//!
//! Dygma ships two disagreeing maps:
//!
//! * the firmware's `key_led_map[4][16]` in `Kaleidoscope-Hardware-Raise.cpp`,
//!   which is a *keymap* coordinate system;
//! * Bazecor's `led_map[5][16]` in `Keymap-ANSI.jsx`, which is the *physical*
//!   grid the GUI uses to colour keys.
//!
//! Measured on the board: LED 0 is Escape (top-left) and LED 26 is left Ctrl
//! (bottom-left). Bazecor's map matches both; the firmware's matches neither. So
//! the physical grid below is Bazecor's, and Bazecor's map is treated as evidence
//! of *shape* rather than gospel: it lists LED 29 twice and omits 40, so at least
//! one cell in it is wrong and the real geometry still needs checking by eye.
//!
//! # The grid
//!
//! Five rows and sixteen columns. Columns 0–7 are the left half, 8–15 the right;
//! the right half's LED indices run in the opposite direction to the left's, so
//! the two halves mirror each other. `None` means no key at that position.

use crate::theme::Rgb;

/// Rows in the physical grid. Four letter rows plus the bottom modifier row.
pub const ROWS: usize = 5;

/// Columns in the physical grid: eight per half.
pub const COLUMNS: usize = 16;

/// Which LED sits at each physical position, or `None` where there is no key.
///
/// Transcribed from Bazecor's `led_map`, itself a 5×16 grid of LED indices.
pub const PHYSICAL_MAP: [[Option<u8>; COLUMNS]; ROWS] = [
    [
        Some(0),
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(6),
        None,
        None,
        Some(39),
        Some(38),
        Some(37),
        Some(36),
        Some(35),
        Some(34),
        Some(33),
    ],
    [
        Some(7),
        Some(8),
        Some(9),
        Some(10),
        Some(11),
        Some(12),
        None,
        None,
        None,
        Some(47),
        Some(46),
        Some(45),
        Some(44),
        Some(43),
        Some(42),
        Some(41),
    ],
    [
        Some(13),
        Some(14),
        Some(15),
        Some(16),
        Some(17),
        Some(18),
        None,
        None,
        None,
        Some(54),
        Some(53),
        Some(52),
        Some(51),
        Some(50),
        Some(49),
        Some(48),
    ],
    [
        Some(19),
        Some(20),
        Some(21),
        Some(22),
        Some(23),
        Some(24),
        Some(25),
        None,
        None,
        None,
        Some(60),
        Some(59),
        Some(58),
        Some(57),
        Some(56),
        Some(55),
    ],
    [
        Some(26),
        Some(27),
        Some(28),
        Some(29),
        Some(30),
        None,
        Some(31),
        Some(32),
        Some(68),
        Some(67),
        Some(66),
        Some(65),
        Some(64),
        Some(63),
        Some(62),
        Some(61),
    ],
];

/// Which half of the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Left,
    Right,
}

impl Half {
    pub const fn name(self) -> &'static str {
        match self {
            Half::Left => "left",
            Half::Right => "right",
        }
    }
}

/// Every LED index the grid claims, in row-major order.
pub fn key_leds() -> Vec<u8> {
    PHYSICAL_MAP
        .iter()
        .flatten()
        .filter_map(|led| *led)
        .collect()
}

/// The LED at a physical position.
pub fn led_at(row: usize, column: usize) -> Option<u8> {
    PHYSICAL_MAP.get(row)?.get(column).copied().flatten()
}

/// The letter rows only: the four rows above the bottom modifier row.
const LETTER_ROWS: std::ops::Range<usize> = 0..4;

/// The LED carrying each band on one half, in band order.
///
/// ADR-0003's shape survives but its arithmetic does not: a half has 26 or 27
/// letter keys, not 28, because the physical grid has holes where a column has no
/// key. So bands snake from the inner column outwards through the columns that
/// exist, and the few bands past the end of a half simply have no key there.
pub fn band_leds(half: Half) -> Vec<u8> {
    // Inner column first: 6 on the left, 9 on the right.
    let columns: Vec<usize> = match half {
        Half::Left => (0..=6).rev().collect(),
        Half::Right => (9..=15).collect(),
    };
    let mut leds = Vec::new();
    for (step, column) in columns.into_iter().enumerate() {
        let mut rows: Vec<usize> = LETTER_ROWS
            .filter(|row| led_at(*row, column).is_some())
            .collect();
        // Boustrophedon: down one column, back up the next, so neighbouring
        // bands stay on physically adjacent keys.
        if !step.is_multiple_of(2) {
            rows.reverse();
        }
        for row in rows {
            leds.push(led_at(row, column).expect("filtered above"));
        }
    }
    leds
}

/// How many bands this half can show.
pub fn band_capacity(half: Half) -> usize {
    band_leds(half).len()
}

/// A static test pattern: bands as a hue ramp, bass at the inner edge.
pub fn gradient(count: usize) -> Vec<Rgb> {
    let mut colors = vec![Rgb::BLACK; count];
    for half in [Half::Left, Half::Right] {
        let last = (band_capacity(half).saturating_sub(1)).max(1) as f32;
        for (band, led) in band_leds(half).iter().enumerate() {
            if let Some(slot) = colors.get_mut(*led as usize) {
                *slot = hue_at((240.0 * band as f32 / last) as u16, 150);
            }
        }
    }
    colors
}

/// A pattern that paints each physical column a different colour, so a photograph
/// shows which keys share a column and where the gaps are.
///
/// Columns are coloured in physical order from the board's left edge, so the
/// photo can be read left to right without knowing the grid.
pub fn column_pattern(count: usize) -> Vec<Rgb> {
    let mut colors = vec![Rgb::BLACK; count];
    for column in 0..COLUMNS {
        let colour = column_colour(column);
        for row in 0..ROWS {
            if let Some(led) = led_at(row, column) {
                if let Some(slot) = colors.get_mut(led as usize) {
                    *slot = colour;
                }
            }
        }
    }
    colors
}

/// A pattern that paints each physical row a different colour.
pub fn row_pattern(count: usize) -> Vec<Rgb> {
    let mut colors = vec![Rgb::BLACK; count];
    for row in 0..ROWS {
        let colour = row_colour(row);
        for column in 0..COLUMNS {
            if let Some(led) = led_at(row, column) {
                if let Some(slot) = colors.get_mut(led as usize) {
                    *slot = colour;
                }
            }
        }
    }
    colors
}

/// The colour of a column.
///
/// Eight well-separated hues, and mirrored columns share one: the board's pairs
/// are 0 with 15, 1 with 14, and so on, so a photograph shows at a glance
/// whether the halves really mirror. Bright enough to read in a photo.
pub fn column_colour(column: usize) -> Rgb {
    let mirrored = if column < 8 {
        column
    } else {
        COLUMNS - 1 - column
    };
    hue_at((mirrored * 45) as u16, 140)
}

pub fn row_colour(row: usize) -> Rgb {
    hue_at((row * 72) as u16, 140)
}

/// Fully saturated colour at `degrees` around the wheel, at half brightness so
/// neighbouring hues stay distinguishable in a photograph.
fn hue_at(degrees: u16, value: u8) -> Rgb {
    let sector = (degrees % 360) as f32 / 60.0;
    let fraction = sector - sector.floor();
    let rising = (fraction * value as f32) as u8;
    let falling = value - rising;
    match sector as u32 {
        0 => Rgb::new(value, rising, 0),
        1 => Rgb::new(falling, value, 0),
        2 => Rgb::new(0, value, rising),
        3 => Rgb::new(0, falling, value),
        4 => Rgb::new(rising, 0, value),
        _ => Rgb::new(value, 0, falling),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_led_appears_exactly_once() {
        // Bazecor's raw map lists LED 29 twice and places 40 where the board has
        // no key. The photograph settles it: 68 keys, and LED 40 unused.
        let leds = key_leds();
        assert_eq!(leds.len(), 68, "68 key LEDs");
        let mut sorted = leds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), leds.len(), "no LED is on two keys");
        assert!(!leds.contains(&40), "LED 40 sits on no key");
    }

    #[test]
    fn the_halves_split_where_bazecor_says() {
        // Bazecor's device definition: ledsLeft = 0..33, ledsRight = 33..69.
        for row in 0..ROWS {
            for column in 0..8 {
                if let Some(led) = led_at(row, column) {
                    assert!(led < 33, "left half column {column} holds LED {led}");
                }
            }
            for column in 8..COLUMNS {
                if let Some(led) = led_at(row, column) {
                    assert!(led >= 33, "right half column {column} holds LED {led}");
                }
            }
        }
    }

    #[test]
    fn the_two_halves_are_mirrored_in_led_order() {
        // Left columns ascend with LED index; right columns descend, which is how
        // the halves end up looking like mirror images.
        let left: Vec<u8> = (0..8).filter_map(|column| led_at(0, column)).collect();
        let right: Vec<u8> = (8..16).filter_map(|column| led_at(0, column)).collect();
        assert!(
            left.windows(2).all(|pair| pair[0] < pair[1]),
            "left row 0 should ascend: {left:?}"
        );
        assert!(
            right.windows(2).all(|pair| pair[0] > pair[1]),
            "right row 0 should descend: {right:?}"
        );
    }

    #[test]
    fn the_two_known_anchors_are_where_the_board_says() {
        // Measured by eye: LED 0 is Escape, top-left; LED 26 is left Ctrl,
        // bottom-left. These are the two facts every other cell hangs on.
        assert_eq!(led_at(0, 0), Some(0), "Escape is the top-left key");
        assert_eq!(led_at(4, 0), Some(26), "left Ctrl is the bottom-left key");
    }

    #[test]
    fn each_half_has_as_many_bands_as_the_photo_shows_letter_keys() {
        // Photographed: left 7+6+6+7, right 7+7+7+6.
        assert_eq!(band_capacity(Half::Left), 26);
        assert_eq!(band_capacity(Half::Right), 27);
    }

    #[test]
    fn bass_starts_on_the_inner_column() {
        // The inner keys are "6" on the left and "7" on the right.
        assert_eq!(band_leds(Half::Left)[0], 6, "left band 0 is the 6 key");
        assert_eq!(band_leds(Half::Right)[0], 39, "right band 0 is the 7 key");
    }

    #[test]
    fn bands_stay_on_letter_rows() {
        let letter: Vec<u8> = (0..4)
            .flat_map(|row| (0..COLUMNS).filter_map(move |column| led_at(row, column)))
            .collect();
        for half in [Half::Left, Half::Right] {
            for led in band_leds(half) {
                assert!(letter.contains(&led), "LED {led} is not a letter-row key");
            }
        }
    }

    #[test]
    fn bands_are_adjacent_or_one_column_apart() {
        // Within a column the step is one key; at a column change the band moves
        // to the neighbouring column, which is one key away physically.
        for half in [Half::Left, Half::Right] {
            let leds = band_leds(half);
            let mut unique = leds.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), leds.len(), "{} reuses a key", half.name());
        }
    }

    #[test]
    fn a_gradient_lights_only_letter_keys() {
        let colors = gradient(132);
        for (led, colour) in colors.iter().enumerate().skip(69) {
            assert_eq!(*colour, Rgb::BLACK, "underglow {led} stays dark");
        }
        let lit = colors.iter().filter(|c| **c != Rgb::BLACK).count();
        assert_eq!(lit, 26 + 27, "53 band keys across the two halves");
    }

    #[test]
    fn a_column_pattern_colours_every_key_in_that_column_alike() {
        let colors = column_pattern(132);
        for column in 0..COLUMNS {
            let mut seen = Vec::new();
            for row in 0..ROWS {
                if let Some(led) = led_at(row, column) {
                    seen.push(colors[led as usize]);
                }
            }
            seen.dedup();
            assert_eq!(seen.len(), 1, "column {column} is not one colour");
        }
    }

    #[test]
    fn a_column_pattern_leaves_underglow_dark() {
        let colors = column_pattern(132);
        for (led, colour) in colors.iter().enumerate().skip(69) {
            assert_eq!(*colour, Rgb::BLACK, "underglow LED {led} must stay dark");
        }
    }

    #[test]
    fn a_row_pattern_colours_every_key_in_that_row_alike() {
        let colors = row_pattern(132);
        for row in 0..ROWS {
            let mut seen = Vec::new();
            for column in 0..COLUMNS {
                if let Some(led) = led_at(row, column) {
                    seen.push(colors[led as usize]);
                }
            }
            seen.dedup();
            assert_eq!(seen.len(), 1, "row {row} is not one colour");
        }
    }

    #[test]
    fn every_pattern_survives_a_smaller_board() {
        // A board reporting fewer LEDs than the grid describes must not panic.
        for colours in [column_pattern(8), row_pattern(8)] {
            assert_eq!(colours.len(), 8);
        }
    }

    #[test]
    fn mirrored_columns_share_a_colour_and_neighbours_do_not() {
        for column in 0..8 {
            assert_eq!(
                column_colour(column),
                column_colour(COLUMNS - 1 - column),
                "columns {column} and {} should mirror",
                COLUMNS - 1 - column
            );
        }
        let mut seen = Vec::new();
        for column in 0..8 {
            let colour = column_colour(column);
            assert!(!seen.contains(&colour), "column {column} repeats a colour");
            seen.push(colour);
        }
    }

    #[test]
    fn pattern_colours_are_bright_enough_to_photograph() {
        for column in 0..COLUMNS {
            let colour = column_colour(column);
            let brightest = colour.r.max(colour.g).max(colour.b);
            assert!(brightest >= 130, "column {column} is too dim: {colour}");
        }
    }
}
