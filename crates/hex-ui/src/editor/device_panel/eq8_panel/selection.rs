//! Eight fixed logical band slots share selection across the active lane/bank.
//! An ordinary selected-node press collapses a group only on stationary release.
use super::*;

pub(super) fn mask(panel: &Eq8PanelState) -> u8 {
    panel.selected_bands.unwrap_or_else(|| {
        if panel.selected_band < BAND_COUNT {
            1 << panel.selected_band
        } else {
            0
        }
    })
}
pub(super) fn marquee_event(
    panel: &mut Eq8PanelState,
    event: &hex_widgets::ResponseGraphMarqueeEvent,
) {
    use hex_widgets::ResponseGraphMarqueePhase as Phase;
    if event.phase == Phase::Cancel
        && let Some((primary, mask)) = panel.marquee_before.take()
    {
        panel.selected_band = primary;
        panel.selected_bands = mask;
        panel.collapse_band = None;
        return;
    }
    if event.phase == Phase::Change && panel.marquee_before.is_none() {
        panel.marquee_before = Some((panel.selected_band, panel.selected_bands));
    }
    marquee(panel, &event.selected);
    if event.phase != Phase::Change {
        panel.marquee_before = None;
    }
}
pub(super) fn marquee(panel: &mut Eq8PanelState, selected: &[u32]) {
    let bits = selected
        .iter()
        .filter(|id| **id < BAND_COUNT as u32)
        .fold(0, |mask, id| mask | (1 << *id));
    panel.selected_bands = Some(bits);
    panel.collapse_band = None;
    if bits != 0 && (panel.selected_band >= BAND_COUNT || bits & (1 << panel.selected_band) == 0) {
        panel.selected_band = bits.trailing_zeros() as usize;
    }
}
pub(super) fn select(panel: &mut Eq8PanelState, band: usize, shift: bool) {
    if band >= BAND_COUNT {
        return;
    }
    let bits = mask(panel);
    panel.collapse_band = None;
    if shift {
        panel.selected_bands = Some(bits ^ (1 << band));
        panel.selected_band = band;
    } else if bits.count_ones() > 1 && bits & (1 << band) != 0 {
        panel.collapse_band = Some(band);
        panel.selected_band = band;
    } else {
        panel.selected_band = band;
        panel.selected_bands = None;
    }
}
pub(super) fn release(panel: &mut Eq8PanelState, band: usize) {
    if panel.collapse_band.take() == Some(band) {
        panel.selected_bands = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_marquee_restores_primary_and_complete_membership() {
        use hex_widgets::{ResponseGraphMarqueeEvent as Event, ResponseGraphMarqueePhase as Phase};
        let mut panel = Eq8PanelState::default();
        marquee(&mut panel, &[2, 4]);
        panel.selected_band = 4;
        marquee_event(
            &mut panel,
            &Event {
                phase: Phase::Change,
                selected: vec![1, 3],
                modifiers: Default::default(),
            },
        );
        assert_eq!(mask(&panel), 10);
        marquee_event(
            &mut panel,
            &Event {
                phase: Phase::Cancel,
                selected: vec![2, 4],
                modifiers: Default::default(),
            },
        );
        assert_eq!(mask(&panel), 20);
        assert_eq!(panel.selected_band, 4);
        assert!(panel.marquee_before.is_none());
    }
    #[test]
    fn marquee_masks_fixed_slots_and_click_collapse_waits_for_release() {
        let mut p = Eq8PanelState::default();
        assert_eq!(mask(&p), 1);
        marquee(&mut p, &[1, 2, 2, 99]);
        assert_eq!(mask(&p), 6);
        select(&mut p, 2, false);
        assert_eq!(mask(&p), 6);
        release(&mut p, 2);
        assert_eq!(mask(&p), 4);
        marquee(&mut p, &[]);
        assert_eq!(mask(&p), 0);
        select(&mut p, 1, true);
        assert_eq!(mask(&p), 2);
        select(&mut p, 1, true);
        assert_eq!(mask(&p), 0);
    }
}
