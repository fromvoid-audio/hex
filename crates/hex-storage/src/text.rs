//! Verbatim spans in status and error text.
//!
//! App-owned text is displayed lowercase, but paths and other user or OS names
//! inside a message are case-sensitive. A producer wraps each such value with
//! [`verbatim`], which surrounds it with the Unicode first-strong-isolate and
//! pop-directional-isolate marks (U+2068, U+2069). Consumers lowercase only the
//! text outside those spans with [`lowercase_outside_verbatim`], and a display or
//! terminal strips the marks with [`plain`] as its final step.
//!
//! An unmatched start mark keeps the rest of the message verbatim; a stray end
//! mark is ordinary text. Neither ever discards characters other than the marks.

const START: char = '\u{2068}';
const END: char = '\u{2069}';

/// Marks `value` as case-sensitive text inside a message.
pub fn verbatim(value: impl std::fmt::Display) -> String {
    format!("{START}{value}{END}")
}

/// Lowercases text outside verbatim spans and keeps the marks for a later display.
pub fn lowercase_outside_verbatim(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            START => depth += 1,
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

/// Removes the verbatim marks for display or terminal output.
pub fn plain(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(*character, START | END))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_text_outside_verbatim_spans_is_lowercased() {
        let message = format!(
            "Project Changed At {} · Retry {}",
            verbatim("/Users/Me/My Songs/A.hex"),
            verbatim("B")
        );
        let lowered = lowercase_outside_verbatim(&message);
        assert_eq!(
            plain(&lowered),
            "project changed at /Users/Me/My Songs/A.hex · retry B"
        );
        assert_eq!(lowercase_outside_verbatim(&lowered), lowered, "idempotent");
        // Nested and unmatched marks never drop characters.
        assert_eq!(plain(&lowercase_outside_verbatim("A\u{2069}B")), "ab");
        assert_eq!(plain(&lowercase_outside_verbatim("A\u{2068}B")), "aB");
        let nested = format!("X{}Y", verbatim(format!("P{}Q", verbatim("R"))));
        assert_eq!(plain(&lowercase_outside_verbatim(&nested)), "xPRQy");
    }
}
