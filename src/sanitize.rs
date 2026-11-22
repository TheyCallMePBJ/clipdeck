//! Making arbitrary clipboard text safe to draw.
//!
//! Clipboard contents can include control characters, such as the escape
//! sequences in colored terminal output. Written to the terminal as-is,
//! those would move the cursor or change colors and corrupt the UI, so they
//! are replaced with visible stand-ins.

/// Replaces control characters so `text` can be drawn safely. Newlines are
/// kept, tabs become `tab`, carriage returns are dropped (so Windows line
/// endings look normal), and other control characters become their Unicode
/// "control picture", e.g. ESC becomes `␛`.
pub fn sanitize(text: &str, tab: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str(tab),
            '\r' => {}
            // C0 controls (0x00-0x1F) have pictures at U+2400 + code.
            '\0'..='\u{1f}' => out.push(char::from_u32(0x2400 + c as u32).unwrap_or('�')),
            '\u{7f}' => out.push('␡'),
            c if c.is_control() => out.push('�'),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(sanitize("hello\nworld ✓", "    "), "hello\nworld ✓");
    }

    #[test]
    fn escape_sequences_become_visible() {
        assert_eq!(sanitize("\x1b[31mred\x1b[0m", " "), "␛[31mred␛[0m");
    }

    #[test]
    fn tabs_crs_and_other_controls() {
        assert_eq!(sanitize("a\tb", "  "), "a  b");
        assert_eq!(sanitize("a\r\nb", " "), "a\nb");
        assert_eq!(sanitize("\0\x07\x7f\u{85}", " "), "␀␇␡�");
    }
}
