//! Every string from git is untrusted. Nothing reaches a cell without passing here.
use unicode_width::UnicodeWidthChar;

pub fn sanitize(text: &str) -> String {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for ch in text.chars() {
        match ch {
            '\t' => {
                let pad = 8 - (column % 8);
                out.extend(std::iter::repeat_n(' ', pad));
                column += pad;
            }
            c if (c as u32) < 0x20 => {
                out.push(char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'));
                column += 1;
            }
            '\x7f' => {
                out.push('\u{fffd}');
                column += 1;
            }
            c if (0x80..=0x9f).contains(&(c as u32)) => {
                out.push('\u{fffd}');
                column += 1;
            }
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
                out.push('\u{fffd}');
                column += 1;
            }
            c => {
                out.push(c);
                column += c.width().unwrap_or(0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn replaces_c0_controls_with_control_pictures() {
        assert_eq!(sanitize("a\x1b[31mb"), "a\u{241b}[31mb");
        assert_eq!(sanitize("x\x00y\x07z"), "x\u{2400}y\u{2407}z");
    }

    #[test]
    fn expands_tabs_to_the_next_multiple_of_eight() {
        assert_eq!(sanitize("\tx"), "        x");
        assert_eq!(sanitize("ab\tx"), "ab      x");
    }

    #[test]
    fn replaces_del_and_c1_with_the_replacement_character() {
        assert_eq!(sanitize("a\x7fb\u{9b}c"), "a\u{fffd}b\u{fffd}c");
    }

    #[test]
    fn strips_a_trailing_newline_only() {
        assert_eq!(sanitize("line\n"), "line");
        assert_eq!(sanitize("a\nb"), "a\u{240a}b");
    }

    #[test]
    fn replaces_bidi_overrides_and_isolates_with_the_replacement_character() {
        for code in (0x202a..=0x202e).chain(0x2066..=0x2069) {
            let control = char::from_u32(code).unwrap();
            assert_eq!(
                sanitize(&format!("a{control}\tx")),
                "a\u{fffd}      x",
                "U+{code:04X} must become a one-cell replacement"
            );
        }
        assert_eq!(
            sanitize("\u{200e}\u{200f}\u{061c}"),
            "\u{200e}\u{200f}\u{061c}"
        );
    }
}
