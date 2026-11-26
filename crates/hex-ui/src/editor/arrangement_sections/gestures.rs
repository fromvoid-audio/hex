//! Section pointer capture, drag/resize gestures, request submission, menus and keyboard actions.

mod actions;
mod pointer;
mod submission;

#[cfg(test)]
mod tests {
    use super::super::Sections;

    #[test]
    fn sections_default_starts_idle_without_pending_or_cursor() {
        let sections = Sections::default();
        assert!(sections.gesture.is_none());
        assert!(sections.pending.is_none());
        assert!(sections.queued.is_none());
        assert!(sections.cursor.is_none());
        assert!(sections.selected.is_none());
    }
}
