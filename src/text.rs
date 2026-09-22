pub(crate) fn sanitize(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch != '\t' && ch.is_control() {
                '\u{fffd}'
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn diagnostics_replace_controls_except_tab_and_preserve_printable_unicode() {
        for code in (0..=0x1f).chain(0x7f..=0x9f) {
            let control = char::from_u32(code).unwrap();
            let expected = if control == '\t' { '\t' } else { '\u{fffd}' };
            assert_eq!(sanitize(&format!("a{control}b")), format!("a{expected}b"));
        }
        assert_eq!(sanitize("é 猫\tplain"), "é 猫\tplain");
        assert_eq!(
            sanitize("host\x1b[31m\nerror"),
            "host\u{fffd}[31m\u{fffd}error"
        );
    }
}
