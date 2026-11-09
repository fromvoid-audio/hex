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
    let nested = format!("X{}Y", verbatim(format!("P{}Q", verbatim("R"))));
    assert_eq!(plain(&lowercase_outside_verbatim(&nested)), "xPRQy");
}
