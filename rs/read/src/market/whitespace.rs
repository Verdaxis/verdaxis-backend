//! Python's Unicode whitespace contract for string qualification.

/// Matches Python `str.isspace()` (Unicode Zs or bidi WS/B/S), including the
/// four information separators that Rust's Unicode White_Space omits.
pub fn is_python_whitespace(character: char) -> bool {
    matches!(character,
        '\u{0009}'..='\u{000d}' | '\u{001c}'..='\u{0020}' | '\u{0085}' |
        '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' |
        '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}
