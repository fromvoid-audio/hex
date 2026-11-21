//! Clock transfer for an exact hosted effect move between two direct audio rows.
#[cfg(feature = "instrument-test-support")]
use super::TransportStamp;
use super::{SignalRuntime, error};
#[cfg(feature = "instrument-test-support")]
use crate::Frame;
use crate::Invalid;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostedEffectClock {
    old_dry: u8,
    new_dry: u8,
    delay: u16,
}

impl SignalRuntime {
    pub(crate) fn hosted_effect_clock_from(
        &self,
        previous: &Self,
        donor: usize,
        receiver: usize,
    ) -> Option<HostedEffectClock> {
        let delay = previous.horizon();
        if donor == receiver
            || delay == 0
            || delay > 512
            || [self, previous].into_iter().any(|runtime| {
                !Arc::ptr_eq(&runtime.graph, &runtime.delay_graph)
                    || !runtime.delays.is_closed()
                    || !runtime.graph.independent_main_routes()
                    || !runtime.graph.main_devices.is_empty()
                    || runtime.graph.tracks.len() != 2
                    || runtime.horizon() != delay
                    || runtime.graph.main_input_arrival() != delay
            })
            || !self.graph.tracks.iter().map(|track| track.id).eq(previous
                .graph
                .tracks
                .iter()
                .map(|track| track.id))
        {
            return None;
        }

        let check_alignment = |runtime: &Self, wet, dry| {
            let (_, wet_arrival) = runtime.graph.resident_main_alignment(wet)?;
            let (dry, dry_arrival) = runtime.graph.resident_main_alignment(dry)?;
            if wet_arrival != delay || dry_arrival != 0 {
                return None;
            }
            let dry = *runtime.graph.pre_main_edges.get(dry)?;
            runtime
                .graph
                .plan
                .edges()
                .iter()
                .enumerate()
                .all(|(index, edge)| edge.delay_frames == if index == dry { delay } else { 0 })
                .then_some(dry)
        };
        Some(HostedEffectClock {
            old_dry: u8::try_from(check_alignment(previous, donor, receiver)?).ok()?,
            new_dry: u8::try_from(check_alignment(self, receiver, donor)?).ok()?,
            delay: u16::try_from(delay).ok()?,
        })
    }

    pub(crate) fn hosted_effect_clock_ready(&self, clock: HostedEffectClock) -> bool {
        self.delays.history_ready()
            && self
                .delays
                .edge_history_is_silent(usize::from(clock.old_dry))
    }

    pub(crate) fn can_transfer_hosted_effect_clock_from(
        &self,
        previous: &Self,
        donor: usize,
        receiver: usize,
        expected: HostedEffectClock,
    ) -> bool {
        self.hosted_effect_clock_from(previous, donor, receiver) == Some(expected)
            && previous.hosted_effect_clock_ready(expected)
            && self
                .delays
                .edge_history_is_silent(usize::from(expected.new_dry))
            && self.delays.latest_stamp().is_none()
    }

    pub(crate) fn transfer_hosted_effect_clock_from(
        &mut self,
        previous: &Self,
        donor: usize,
        receiver: usize,
        expected: HostedEffectClock,
    ) -> Result<(), Invalid> {
        if !self.can_transfer_hosted_effect_clock_from(previous, donor, receiver, expected) {
            return Err(Invalid("hosted effect alignment is not ready"));
        }
        // The delayed edge has a different signal owner after the cut.
        // Only the original shared timeline stamps cross to the fresh ring.
        self.delays
            .copy_stamp_history_from(&previous.delays)
            .map_err(error)
    }
}

#[cfg(feature = "instrument-test-support")]
pub(crate) fn probe_hosted_effect_clock(
    mut old: SignalRuntime,
    mut next: SignalRuntime,
) -> Result<(), Invalid> {
    let clock = next
        .hosted_effect_clock_from(&old, 0, 1)
        .ok_or(Invalid("hosted effect alignment does not match"))?;
    assert!(next.hosted_effect_clock_from(&old, 1, 0).is_none());
    let stamp = |frame| TransportStamp {
        authored_clock_epoch: std::num::NonZeroU64::new(1),
        tempo_micro_bpm: None,
        performance_frame: frame,
        transport_is_running: true,
        arrangement_frame: frame + 256,
        loop_pass: 0,
        performance_beat_raw: frame,
        arrangement_beat_raw: None,
        meter: None,
        cycle_beats_raw: None,
    };
    let render = |runtime: &mut SignalRuntime, frame, edge: u8, input| {
        let mut output = [[0.0; 2]];
        runtime.delays.begin_transaction().map_err(error)?;
        runtime
            .delays
            .push_transport_stamp(stamp(frame))
            .map_err(error)?;
        runtime
            .delays
            .process_edge(usize::from(edge), &[input], &mut output)
            .map_err(error)?;
        Ok::<_, Invalid>(output)
    };

    assert!(
        next.transfer_hosted_effect_clock_from(&old, 0, 1, clock)
            .is_err()
    );
    let first: Frame = [0.125, -0.25];
    let delay = u64::from(clock.delay);
    for frame in 0..delay * 2 {
        let input = if frame == delay { first } else { [0.0; 2] };
        assert_eq!(render(&mut old, frame, clock.old_dry, input)?, [[0.0; 2]]);
        old.delays.commit_transaction().map_err(error)?;
        assert!(
            next.transfer_hosted_effect_clock_from(&old, 0, 1, clock)
                .is_err()
        );
    }
    assert!(next.delays.latest_stamp().is_none());
    assert_eq!(
        render(&mut old, delay * 2, clock.old_dry, [0.0; 2])?,
        [first]
    );
    assert!(
        next.transfer_hosted_effect_clock_from(&old, 0, 1, clock)
            .is_err()
    );
    old.delays.rollback_transaction().map_err(error)?;
    assert!(
        next.transfer_hosted_effect_clock_from(&old, 0, 1, clock)
            .is_err()
    );
    assert_eq!(
        render(&mut old, delay * 2, clock.old_dry, [0.0; 2])?,
        [first]
    );
    old.delays.commit_transaction().map_err(error)?;
    next.transfer_hosted_effect_clock_from(&old, 0, 1, clock)?;
    for age in 0..=u32::from(clock.delay) {
        assert_eq!(
            next.delays.stamp_at_delay(age),
            old.delays.stamp_at_delay(age)
        );
    }
    let second: Frame = [-0.5, 0.25];
    for index in 0..=delay {
        let output = render(&mut next, delay * 2 + index + 1, clock.new_dry, second)?;
        next.delays.commit_transaction().map_err(error)?;
        assert_eq!(output, [if index < delay { [0.0; 2] } else { second }]);
    }
    Ok(())
}
