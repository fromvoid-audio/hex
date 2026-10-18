//! Caller-selected text presentation; editing, parsing and storage retain source text.

/// Visual casing only. The product chooses its policy at the styling boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextCase {
    Preserve,
    Lowercase,
}

impl TextCase {
    pub fn present(self, source: &str) -> String {
        match self {
            Self::Preserve => source.to_owned(),
            Self::Lowercase => source.to_lowercase(),
        }
    }

    /// Map a source UTF-8 boundary into the shaped presentation, including case
    /// expansions. Native selection and clipboard offsets remain source-based.
    pub fn display_offset(self, source: &str, offset: usize) -> usize {
        if self == Self::Preserve {
            return source
                .char_indices()
                .map(|(index, _)| index)
                .chain([source.len()])
                .take_while(|index| *index <= offset)
                .last()
                .unwrap_or(0);
        }
        source
            .char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= offset)
            .map(|(_, character)| character.to_lowercase().map(char::len_utf8).sum::<usize>())
            .sum()
    }

    /// Map a shaped UTF-8 hit to a source boundary. An interior hit in a case
    /// expansion resolves to its nearest source endpoint, never an invalid byte.
    pub fn source_offset(self, source: &str, offset: usize) -> usize {
        if self == Self::Preserve {
            return source
                .char_indices()
                .map(|(index, _)| index)
                .chain([source.len()])
                .take_while(|index| *index <= offset)
                .last()
                .unwrap_or(0);
        }
        let mut display_start = 0;
        for (index, character) in source.char_indices() {
            let length = character.to_lowercase().map(char::len_utf8).sum::<usize>();
            if offset <= display_start {
                return index;
            }
            if offset < display_start + length {
                return if (offset - display_start) * 2 < length {
                    index
                } else {
                    index + character.len_utf8()
                };
            }
            display_start += length;
        }
        source.len()
    }
}

#[cfg(test)]
mod tests {
    use super::TextCase;

    #[test]
    fn lowercase_presentation_maps_source_boundaries_without_changing_values() {
        for source in [
            "Verse S 🎹",
            "İA",
            "Ⱥİ🎛Z",
            "ΟΣ",
            "A\u{301}",
            "/Samples/Kick.WAV",
            "1E3",
        ] {
            let display = TextCase::Lowercase.present(source);
            assert_eq!(display, source.to_lowercase());
            for offset in source
                .char_indices()
                .map(|(index, _)| index)
                .chain([source.len()])
            {
                let shown = TextCase::Lowercase.display_offset(source, offset);
                assert!(display.is_char_boundary(shown));
                assert_eq!(TextCase::Lowercase.source_offset(source, shown), offset);
            }
            for offset in 0..=display.len() + 1 {
                assert!(source.is_char_boundary(TextCase::Lowercase.source_offset(source, offset)));
            }
            assert_eq!(TextCase::Preserve.present(source), source);
        }
    }
}
