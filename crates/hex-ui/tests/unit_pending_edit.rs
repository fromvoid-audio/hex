//! Pure receipt state tests; no GPUI window or service is created.

#[path = "../src/editor/pending_edit.rs"]
mod receipt;

mod editor {
    mod pending_edit {
        use crate::receipt::*;

        mod tests {
            use super::*;

            #[test]
            fn queued_and_earlier_events_cannot_retire_released_display() {
                let mut receipt = Receipt::new(7);
                assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
                assert!(!receipt.submitted(0));
                assert!(receipt.submitted(2));
                assert!(!receipt.submitted(1));
                assert_eq!(receipt.observe(7, false, false), Disposition::Waiting);
                assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
                // Success/refusal and revision equality are deliberately not inputs:
                // the same exact terminal releases paint for either canonical outcome.
                assert_eq!(receipt.observe(7, false, true), Disposition::Terminal);
                assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
                assert!(!receipt.submitted(1));
            }

            #[test]
            fn project_replacement_invalidates_both_queued_and_submitted_receipts() {
                for submitted in [false, true] {
                    for replaced in [false, true] {
                        let mut receipt = Receipt::new(7);
                        if submitted {
                            assert!(receipt.submitted(3));
                        }
                        assert_eq!(
                            receipt.observe(if replaced { 7 } else { 8 }, replaced, false),
                            Disposition::Invalidated
                        );
                        assert!(!receipt.submitted(1));
                    }
                }
            }
        }
    }
}
