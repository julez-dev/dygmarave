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
/// ADR-0003 asked for a snake *down* columns. That cannot work on this board: the
/// inner column is partial — the left half has keys at its rows 0 and 3 with a
/// hole between — so walking it jumps three rows. The snake therefore runs along
/// **rows**, which are unbroken, alternating direction row by row and always
/// starting at the inner end:
///
/// ```text
/// left, row 0: 6 5 4 3 2 1 0   (inner to outer)
/// left, row 1: 0 1 2 3 4 5     (outer back to inner)
/// left, row 2: 5 4 3 2 1 0     (inner outwards; column 6 has no key here)
/// left, row 3: 0 1 2 3 4 5 6   (outer back to inner)
/// ```
///
/// Rows 0 ↔ 15, 1 ↔ 14 and so on mirror, and the direction alternates the same
/// way on both halves, so the two halves are mirror images and consecutive bands
/// are always neighbouring keys.
pub fn band_leds(half: Half) -> Vec<u8> {
    let inner_to_outer: Vec<usize> = match half {
        Half::Left => (0..=6).rev().collect(),
        Half::Right => (9..=15).collect(),
    };
    let outer_to_inner: Vec<usize> = match half {
        Half::Left => (0..=6).collect(),
        Half::Right => (9..=15).rev().collect(),
    };

    let mut leds = Vec::new();
    for row in LETTER_ROWS {
        let order = if row.is_multiple_of(2) {
            &inner_to_outer
        } else {
            &outer_to_inner
        };
        for column in order {
            if let Some(led) = led_at(row, *column) {
                leds.push(led);
            }
        }
    }
    leds
}

/// How many bands this half can show.
pub fn band_capacity(half: Half) -> usize {
    band_leds(half).len()
}

/// LEDs in the left thumb cluster.
///
/// The key map covers 0–68; everything from 69 up is underglow. The thumbs are
/// the inner-bottom corner of each half's underglow, which is where the physical
/// thumb keys sit. These indices are structural — the exact mapping needs a
/// photograph to confirm — but the logic is independent of which indices are
/// chosen: each half's cluster gets its own channel's level and beat flash.
pub const THUMB_CLUSTER_LEFT: [u8; 4] = [69, 70, 71, 72];

/// LEDs in the right thumb cluster.
pub const THUMB_CLUSTER_RIGHT: [u8; 4] = [128, 129, 130, 131];

/// The LED indices for one half's thumb cluster.
pub fn thumb_leds(half: Half) -> &'static [u8] {
    match half {
        Half::Left => &THUMB_CLUSTER_LEFT,
        Half::Right => &THUMB_CLUSTER_RIGHT,
    }
}

/// A diagnostic pattern for locating the thumb clusters by photograph.
///
/// The thumb indices in [`THUMB_CLUSTER_LEFT`] and [`THUMB_CLUSTER_RIGHT`] are
/// structural guesses: neither Dygma source records which underglow LEDs are the
/// physical thumb keys. This paints the guessed left cluster magenta, the guessed
/// right cluster cyan, and the rest of the underglow dim grey, so a photograph
/// shows both what was guessed and what was left out. Correct the constants from
/// the photo; the detection logic does not depend on which indices are chosen.
pub fn thumb_pattern(count: usize) -> Vec<Rgb> {
    const BACKDROP: Rgb = Rgb::new(20, 20, 20);
    const LEFT: Rgb = Rgb::new(255, 0, 255);
    const RIGHT: Rgb = Rgb::new(0, 255, 255);

    let mut colors = vec![Rgb::BLACK; count];
    // A dim backdrop over the whole underglow, so its extent is visible in the
    // photo even where no thumb is guessed.
    for slot in colors.iter_mut().skip(69) {
        *slot = BACKDROP;
    }
    for led in THUMB_CLUSTER_LEFT {
        if let Some(slot) = colors.get_mut(led as usize) {
            *slot = LEFT;
        }
    }
    for led in THUMB_CLUSTER_RIGHT {
        if let Some(slot) = colors.get_mut(led as usize) {
            *slot = RIGHT;
        }
    }
    colors
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

    /// The table as recorded. Editing one cell fails this until the expectation is
    /// updated too, which is the point: a transposed pair is otherwise invisible.
    #[test]
    fn the_whole_table_matches_the_recorded_expectation() {
        const EXPECTED: [[Option<u8>; COLUMNS]; ROWS] = [
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
        assert_eq!(PHYSICAL_MAP, EXPECTED, "the grid must not change silently");
    }

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
    fn no_band_reuses_a_key() {
        for half in [Half::Left, Half::Right] {
            let leds = band_leds(half);
            let mut unique = leds.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), leds.len(), "{} reuses a key", half.name());
        }
    }

    /// Where a band sits, as (row, column).
    fn band_cell(led: u8) -> (usize, usize) {
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                if led_at(row, column) == Some(led) {
                    return (row, column);
                }
            }
        }
        panic!("LED {led} is not on the grid");
    }

    #[test]
    fn consecutive_bands_sit_on_neighbouring_keys() {
        for half in [Half::Left, Half::Right] {
            let leds = band_leds(half);
            for pair in leds.windows(2) {
                let (row_a, col_a) = band_cell(pair[0]);
                let (row_b, col_b) = band_cell(pair[1]);
                let distance = row_a.abs_diff(row_b) + col_a.abs_diff(col_b);
                assert!(
                    distance <= 1,
                    "{} half: LED {} at ({row_a},{col_a}) jumps {distance} keys to LED {} at ({row_b},{col_b})",
                    half.name(),
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    #[test]
    fn the_snake_turns_at_a_row_end_not_a_column_end() {
        // Row 0 runs inner to outer, row 1 back outward to inner: the turn happens
        // where two rows meet, which is one key apart.
        let leds = band_leds(Half::Left);
        let (last_of_row0_row, last_of_row0_column) = band_cell(leds[6]);
        let (first_of_row1_row, first_of_row1_column) = band_cell(leds[7]);
        assert_eq!(last_of_row0_column, 0, "row 0 ends at the outer edge");
        assert_eq!(
            (last_of_row0_row, last_of_row0_column),
            (0, 0),
            "row 0 ends at the outer column"
        );
        assert_eq!(
            (first_of_row1_row, first_of_row1_column),
            (1, 0),
            "row 1 starts directly below where row 0 ended"
        );
    }

    #[test]
    fn every_left_column_has_its_mirror_on_the_right() {
        // Column c on the left pairs with 15-c on the right. The right half has one
        // extra key in some rows (its inner column reaches further down), but no
        // left key ever lacks a mirrored counterpart.
        for row in LETTER_ROWS {
            let left: Vec<usize> = (0..8)
                .filter(|column| led_at(row, *column).is_some())
                .collect();
            let right: Vec<usize> = (8..COLUMNS)
                .filter(|column| led_at(row, *column).is_some())
                .collect();
            for column in left {
                // One cell has no counterpart: the right half's inner column stops
                // a row earlier than the left's. Recorded rather than skipped, so
                // a second unexplained hole fails this test.
                if (row, column) == (3, 6) {
                    assert!(!right.contains(&(COLUMNS - 1 - column)));
                    continue;
                }
                let mirror = COLUMNS - 1 - column;
                assert!(
                    right.contains(&mirror),
                    "row {row}: left column {column} has no mirror at {mirror}"
                );
            }
        }
    }

    #[test]
    fn the_halves_are_not_perfectly_symmetric_and_that_is_a_board_fact() {
        // Rows hold 7,6,6,7 keys on the left and 7,7,7,6 on the right, so the two
        // halves carry different numbers of bands. Any claim of an exactly
        // symmetric picture would be false; this test keeps the docs honest
        // rather than demanding symmetry the board cannot give.
        assert_eq!(band_capacity(Half::Left), 26);
        assert_eq!(band_capacity(Half::Right), 27);
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
    fn thumb_clusters_are_distinct_underglow_leds() {
        // Each half's cluster must be separate, and must not overlap the key map.
        let key_set: Vec<u8> = key_leds();
        for half in [Half::Left, Half::Right] {
            for led in thumb_leds(half) {
                assert!(
                    !key_set.contains(led),
                    "thumb LED {led} must not be a key LED"
                );
                assert!(*led >= 69, "thumb LED {led} must be underglow");
            }
        }
        // The two clusters must not share LEDs.
        for led in THUMB_CLUSTER_LEFT {
            assert!(
                !THUMB_CLUSTER_RIGHT.contains(&led),
                "LED {led} appears in both clusters"
            );
        }
    }

    #[test]
    fn a_thumb_pattern_lights_only_the_guessed_clusters_brightly() {
        let colors = thumb_pattern(132);
        // The guessed clusters are the only bright LEDs in the underglow.
        for led in THUMB_CLUSTER_LEFT {
            assert_eq!(colors[led as usize], Rgb::new(255, 0, 255));
        }
        for led in THUMB_CLUSTER_RIGHT {
            assert_eq!(colors[led as usize], Rgb::new(0, 255, 255));
        }
        // Key LEDs stay dark.
        for led in key_leds() {
            assert_eq!(
                colors[led as usize],
                Rgb::BLACK,
                "key LED {led} must be dark"
            );
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
