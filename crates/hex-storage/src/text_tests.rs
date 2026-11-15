use crate::text::*;

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
    assert_eq!(
        plain(&lowercase_outside_verbatim("A\u{2068}B\u{2063}x")),
        "aB\u{2063}x"
    );
    let nested = format!("X{}Y", verbatim_composed(format!("P{}Q", verbatim("R"))));
    assert_eq!(plain(&lowercase_outside_verbatim(&nested)), "xPRQy");
}

#[test]
fn verbatim_preserves_literal_direction_and_escape_marks() {
    for value in [
        "/Mixed\u{2069}Case/Output.WAV",
        "/Mixed\u{2068}Case/Output.WAV",
        "/Mixed\u{2063}Case/Output.WAV",
        "\u{2068}\u{2069}\u{2063}\u{2068}\u{2069}MixedCase",
    ] {
        let marked = verbatim(value);
        let lowered = lowercase_outside_verbatim(&format!("Path {marked} Done"));
        assert_eq!(plain(&lowered), format!("path {value} done"));
        assert_eq!(lowercase_outside_verbatim(&lowered), lowered);
        assert_eq!(plain(&marked), value);
    }
}

#[test]
fn composed_spans_and_reserved_character_pairs_round_trip() {
    let alphabet = ['A', 'b', '\u{2068}', '\u{2069}', '\u{2063}'];
    for first in alphabet {
        for second in alphabet {
            let value = format!("{first}{second}Tail");
            let message = format!(
                "Outer {} Done",
                verbatim_composed(format!("P{}Q", verbatim(&value)))
            );
            let lowered = lowercase_outside_verbatim(&message);
            assert_eq!(plain(&lowered), format!("outer P{value}Q done"));
            assert_eq!(lowercase_outside_verbatim(&lowered), lowered);
        }
    }
}
