//! A large block font for the display, in the style of the meter's LCD.

/// Rows in every glyph.
pub const HEIGHT: usize = 5;

/// Renders `text` as `HEIGHT` lines of block characters, one column apart.
/// Characters without a glyph show as `?`.
pub fn render(text: &str) -> [String; HEIGHT] {
    let mut lines: [String; HEIGHT] = Default::default();
    for (i, c) in text.chars().enumerate() {
        let rows = glyph(c);
        for (line, row) in lines.iter_mut().zip(rows) {
            if i > 0 {
                line.push(' ');
            }
            line.push_str(row);
        }
    }
    lines
}

/// The width of `render(text)` in columns.
pub fn width(text: &str) -> usize {
    let glyphs: usize = text.chars().map(|c| glyph(c)[0].chars().count()).sum();
    glyphs + text.chars().count().saturating_sub(1)
}

/// The rows of `c`'s glyph. Segments follow a seven-segment display, so the
/// letters are the ones a meter's LCD can show.
fn glyph(c: char) -> [&'static str; HEIGHT] {
    match c {
        '0' | 'O' => ["████", "█  █", "█  █", "█  █", "████"],
        '1' => ["   █", "   █", "   █", "   █", "   █"],
        '2' => ["████", "   █", "████", "█   ", "████"],
        '3' => ["████", "   █", "████", "   █", "████"],
        '4' => ["█  █", "█  █", "████", "   █", "   █"],
        '5' => ["████", "█   ", "████", "   █", "████"],
        '6' => ["████", "█   ", "████", "█  █", "████"],
        '7' => ["████", "   █", "   █", "   █", "   █"],
        '8' => ["████", "█  █", "████", "█  █", "████"],
        '9' => ["████", "█  █", "████", "   █", "████"],
        '-' => ["    ", "    ", "████", "    ", "    "],
        '.' => [" ", " ", " ", " ", "█"],
        ' ' => ["  ", "  ", "  ", "  ", "  "],
        'A' => ["████", "█  █", "████", "█  █", "█  █"],
        'E' => ["████", "█   ", "████", "█   ", "████"],
        'F' => ["████", "█   ", "████", "█   ", "█   "],
        'H' => ["█  █", "█  █", "████", "█  █", "█  █"],
        'I' => ["  █ ", "  █ ", "  █ ", "  █ ", "  █ "],
        'L' => ["█   ", "█   ", "█   ", "█   ", "████"],
        'n' => ["    ", "    ", "████", "█  █", "█  █"],
        'o' => ["    ", "    ", "████", "█  █", "████"],
        'r' => ["    ", "    ", "████", "█   ", "█   "],
        't' => ["█   ", "█   ", "████", "█   ", "████"],
        'u' => ["    ", "    ", "█  █", "█  █", "████"],
        _ => ["████", "   █", " ███", "    ", " █  "],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the meter's display can produce.
    const DISPLAY_CHARS: &str = "0123456789-.AutoInErFHLO";

    #[test]
    fn eight_is_all_segments() {
        assert_eq!(render("8"), ["████", "█  █", "████", "█  █", "████"]);
    }

    #[test]
    fn glyphs_are_joined_one_column_apart() {
        let lines = render("1.0");
        assert_eq!(lines[0], "   █   ████");
        assert_eq!(lines[4], "   █ █ ████");
    }

    #[test]
    fn every_display_character_has_a_glyph() {
        let unknown = render("?");
        for c in DISPLAY_CHARS.chars() {
            let lines = render(&c.to_string());
            let w = lines[0].chars().count();
            assert!(w > 0, "{c}");
            assert!(
                lines.iter().all(|l| l.chars().count() == w),
                "{c} is ragged"
            );
            if c != '8' && c != '0' && c != 'O' {
                assert_ne!(lines, unknown, "{c} falls back to ?");
            }
        }
    }

    #[test]
    fn unknown_characters_show_as_a_question_mark() {
        assert_eq!(render("x"), render("?"));
    }

    #[test]
    fn width_matches_the_rendering() {
        for text in [
            "3.2981", "-0.0012", "OL", "Auto", "InEr", "-----", "EF-H", "",
        ] {
            let lines = render(text);
            assert!(
                lines.iter().all(|l| l.chars().count() == width(text)),
                "{text}"
            );
        }
    }
}
