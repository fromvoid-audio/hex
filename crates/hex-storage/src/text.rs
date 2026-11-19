//! Verbatim spans in status and error text.
//!
//! App-owned text is displayed lowercase, but paths and other user or OS names
//! inside a message are case-sensitive. [`verbatim`] wraps a raw value and
//! escapes literal protocol characters. [`verbatim_composed`] wraps text that
//! already contains marked spans without escaping those spans. Use it only for
//! composed markup, not raw user data. Consumers lowercase only outside spans
//! with [`lowercase_outside_verbatim`]; [`plain`] removes markup at the final
//! display or terminal boundary and restores escaped literal characters.
//!
//! An unmatched start mark keeps the rest of the message verbatim; a stray end
//! mark is ordinary text. The escape mark has protocol meaning only inside a
//! span and before one of the three reserved marks. Repeated normalization of
//! marked text is idempotent. Plain text returned by [`plain`] is final output,
//! not marked text to normalize again.

const START: char = '\u{2068}';
const END: char = '\u{2069}';
const ESCAPE: char = '\u{2063}';

fn reserved(character: char) -> bool {
    matches!(character, START | END | ESCAPE)
}

/// Marks a raw value as case-sensitive, preserving literal protocol marks.
pub fn verbatim(value: impl std::fmt::Display) -> String {
    let formatted = format!("{START}{value}{END}");
    let Some(raw) = formatted
        .strip_prefix(START)
        .and_then(|text| text.strip_suffix(END))
    else {
        return formatted;
    };
    if !raw.chars().any(reserved) {
        return formatted;
    }
    let mut result = String::with_capacity(formatted.len());
    result.push(START);
    for character in raw.chars() {
        if reserved(character) {
            result.push(ESCAPE);
        }
        result.push(character);
    }
    result.push(END);
    result
}

/// Marks already composed text as case-sensitive without re-escaping its spans.
/// Raw user values inside `value` must already have been passed through
/// [`verbatim`]. This explicitly replaces wrapping composed markup in
/// [`verbatim`], which now treats its input as raw text.
pub fn verbatim_composed(value: impl std::fmt::Display) -> String {
    format!("{START}{value}{END}")
}

/// Lowercases text outside verbatim spans and keeps the marks for later display.
pub fn lowercase_outside_verbatim(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth = 0usize;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if depth > 0 && character == ESCAPE && characters.peek().is_some_and(|c| reserved(*c)) {
            out.push(character);
            if let Some(escaped) = characters.next() {
                out.push(escaped);
            }
            continue;
        }
        match character {
            START => depth = depth.saturating_add(1),
            END => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 {
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// Removes span markup at the final display boundary and restores literal marks.
pub fn plain(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth = 0usize;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            START => depth = depth.saturating_add(1),
            END => depth = depth.saturating_sub(1),
            ESCAPE if depth > 0 && characters.peek().is_some_and(|c| reserved(*c)) => {
                if let Some(escaped) = characters.next() {
                    out.push(escaped);
                }
            }
            _ => out.push(character),
        }
    }
    out
}
