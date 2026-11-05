//! The unread count as a small image, for Windows' taskbar overlay: a red circle with the
//! number in white, as Teams and Outlook show theirs. Windows has no badge count; an
//! overlay icon is what it offers. Drawn here, so it needs no font and no image files.

/// Width and height in pixels. Windows scales it to its overlay size, 16 at 100 %.
pub const SIZE: u32 = 32;

const RED: [u8; 3] = [0xd9, 0x30, 0x25];
const WHITE: [u8; 3] = [0xff, 0xff, 0xff];

/// Digits and a plus sign, three pixels wide and five high, one row per line.
const GLYPHS: [(char, [u8; 5]); 11] = [
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
    ('3', [0b111, 0b001, 0b011, 0b001, 0b111]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
    ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b001, 0b001, 0b001]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
    ('+', [0b000, 0b010, 0b111, 0b010, 0b000]),
];

/// What the badge says: the count, or 99+ past that.
pub fn label(count: i64) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// The badge for `count`, as `SIZE`×`SIZE` RGBA pixels. Only for a count above 0: with
/// none, the overlay goes.
pub fn draw(count: i64) -> Vec<u8> {
    let size = SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];

    // The circle, its edge smoothed by sampling each pixel 4×4 times.
    let r = SIZE as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let mut inside = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let dx = x as f32 + (sx as f32 + 0.5) / 4.0 - r;
                    let dy = y as f32 + (sy as f32 + 0.5) / 4.0 - r;
                    if dx * dx + dy * dy <= r * r {
                        inside += 1;
                    }
                }
            }
            let i = (y * size + x) * 4;
            rgba[i..i + 3].copy_from_slice(&RED);
            rgba[i + 3] = (inside * 255 / 16) as u8;
        }
    }

    // The text, as large as fits: fewer characters, bigger pixels.
    let text = label(count);
    let glyphs: Vec<[u8; 5]> = text
        .chars()
        .filter_map(|c| GLYPHS.iter().find(|(g, _)| *g == c).map(|(_, rows)| *rows))
        .collect();
    let scale = match glyphs.len() {
        1 => 4,
        2 => 3,
        _ => 2,
    };
    let gap = scale;
    let width = glyphs.len() * 3 * scale + (glyphs.len().saturating_sub(1)) * gap;
    let height = 5 * scale;
    let (left, top) = ((size - width) / 2, (size - height) / 2);
    for (n, rows) in glyphs.iter().enumerate() {
        let x0 = left + n * (3 * scale + gap);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for y in top + row * scale..top + (row + 1) * scale {
                    for x in x0 + col * scale..x0 + (col + 1) * scale {
                        let i = (y * size + x) * 4;
                        rgba[i..i + 3].copy_from_slice(&WHITE);
                        rgba[i + 3] = 255;
                    }
                }
            }
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * SIZE as usize + x) * 4;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn counts_past_99_read_99_plus() {
        assert_eq!(label(7), "7");
        assert_eq!(label(42), "42");
        assert_eq!(label(99), "99");
        assert_eq!(label(100), "99+");
        assert_eq!(label(12_345), "99+");
    }

    #[test]
    fn a_red_circle_with_white_text_and_clear_corners() {
        for count in [1, 8, 42, 99, 100, 5_000] {
            let rgba = draw(count);
            assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);
            assert_eq!(pixel(&rgba, 0, 0)[3], 0, "corner of {count}");
            assert_eq!(pixel(&rgba, 31, 31)[3], 0, "corner of {count}");
            // Near the edge, inside the circle: red.
            assert_eq!(
                pixel(&rgba, 16, 2),
                [0xd9, 0x30, 0x25, 255],
                "edge of {count}"
            );
            let white = rgba
                .chunks(4)
                .filter(|p| p == &[0xff, 0xff, 0xff, 255])
                .count();
            assert!(white > 20, "text of {count}");
        }
    }

    #[test]
    fn the_text_is_centred_and_fits() {
        // 8 is a full block of three columns at the largest size: 12×20 pixels.
        let rgba = draw(8);
        let white: Vec<(usize, usize)> = (0..32)
            .flat_map(|y| (0..32).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&rgba, x, y) == [0xff, 0xff, 0xff, 255])
            .collect();
        let xs = (
            white.iter().map(|p| p.0).min().unwrap(),
            white.iter().map(|p| p.0).max().unwrap(),
        );
        let ys = (
            white.iter().map(|p| p.1).min().unwrap(),
            white.iter().map(|p| p.1).max().unwrap(),
        );
        assert_eq!(xs, (10, 21));
        assert_eq!(ys, (6, 25));
    }
}
