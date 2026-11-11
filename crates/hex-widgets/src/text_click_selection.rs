//! Local selection policy shared by text fields; no timers or global input hooks.

/// Remembers selection before the first click of a platform double-click pair.
#[derive(Clone, Copy, Default)]
pub struct TextClickSelection {
    before_all: Option<bool>,
}

impl TextClickSelection {
    /// Returns the requested Select All state on the paired click, otherwise
    /// records the pre-gesture state. Rapid pairs may arrive as 1/2 then 3/4.
    pub fn selection_for_down(&mut self, click_count: usize, all: bool) -> Option<bool> {
        if click_count >= 2 && click_count & 1 == 0 {
            self.before_all.take().map(|before| !before)
        } else {
            self.before_all = Some(all);
            None
        }
    }

    /// Invalidate a pair when text, keyboard selection, composition or focus changes.
    pub fn clear(&mut self) {
        self.before_all = None;
    }
}

#[cfg(test)]
mod tests {
    use super::TextClickSelection;

    #[test]
    fn invalidated_pair_does_not_invent_a_second_click() {
        for count in [1, 3] {
            for before in [false, true] {
                for current in [false, true] {
                    let mut policy = TextClickSelection::default();
                    assert_eq!(policy.selection_for_down(count, before), None);
                    policy.clear();
                    assert_eq!(policy.selection_for_down(count + 1, current), None);
                    assert_eq!(policy.selection_for_down(count + 2, current), None);
                    assert_eq!(
                        policy.selection_for_down(count + 3, !current),
                        Some(!current)
                    );
                }
            }
        }
    }
}
