use allocation_counter::measure;
use hex_engine::processor_midi_routing::{
    ProcessorMidiRouteTable, ProcessorMidiRouter, ProcessorMidiSource,
};
use hex_lattice::{
    ForeignNote, ForeignNoteKind, ForeignOutputEvents, ForeignProcessorNode, ForeignProcessors,
    NativeInstrumentClock, PreparedForeignProcessor, ProcessorChainOwner,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use std::num::NonZeroU32;

struct EmittingForeignStub {
    emitted: ForeignOutputEvents,
}

impl ForeignProcessors for EmittingForeignStub {
    fn process_segment(
        &mut self,
        _node: ForeignProcessorNode<'_>,
        _frame_offset: usize,
        _frames: &mut [[f32; 2]],
        _clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Ok(())
    }

    fn drain_output_notes(
        &mut self,
        _node: ForeignProcessorNode<'_>,
        out: &mut ForeignOutputEvents,
    ) -> Result<usize, DeviceContractError> {
        let mut count = 0;
        for &note in self.emitted.as_slice() {
            if out.push(note) {
                count += 1;
            }
        }
        self.emitted.clear();
        Ok(count)
    }

    fn reject_callback(&mut self) {
        self.emitted.clear();
    }
}

#[test]
fn processor_midi_output_routing_allocates_nothing_on_callback() {
    let source = ProcessorMidiSource {
        track_id: 1,
        device_id: 77,
    };
    let table = ProcessorMidiRouteTable::default()
        .with_route(source, 2, true)
        .unwrap();
    let mut router = ProcessorMidiRouter::prepare(&table);
    let config = NativeDeviceState::new(DeviceTypeId::from_static("test.vst3"), 1, vec![]).unwrap();
    let prepared = PreparedForeignProcessor::new(64, 0, 128).unwrap();
    let node = ForeignProcessorNode {
        owner: ProcessorChainOwner::Track(1),
        device_id: 77,
        device_type: DeviceTypeId::from_static("test.vst3"),
        sample_rate: 48_000,
        configuration: &config,
        prepared,
        automation: None,
        bypassed: false,
        input_gain: 1.0,
        output_gain: 1.0,
    };
    let mut stub = EmittingForeignStub {
        emitted: ForeignOutputEvents::new(),
    };
    let mut out = ForeignOutputEvents::new();

    let alloc = measure(|| {
        stub.emitted.clear();
        assert!(stub.emitted.push(ForeignNote {
            frame: 8,
            channel: 0,
            pitch: 64,
            kind: ForeignNoteKind::On,
            value: 100,
            note_id: NonZeroU32::new(11),
            is_live: false,
        }));
        assert!(stub.emitted.push(ForeignNote {
            frame: 24,
            channel: 0,
            pitch: 64,
            kind: ForeignNoteKind::PolyPressure,
            value: 72,
            note_id: NonZeroU32::new(11),
            is_live: false,
        }));

        router.begin_block();
        let routed = router
            .collect_from_foreign(source, node, &mut stub, 64)
            .unwrap();
        assert_eq!(routed, 2);
        out.clear();
        assert_eq!(router.deliver_to_destination(2, &mut out).unwrap(), 2);
        assert_eq!(router.held_note_count(2), 1);

        out.clear();
        assert_eq!(
            router.flush_releases_for_destination(2, &mut out).unwrap(),
            1
        );
        assert_eq!(router.held_note_count(2), 0);
    });
    assert_eq!(alloc.count_total, 0);
    assert_eq!(alloc.bytes_total, 0);
    assert_eq!(alloc.count_current, 0);
    assert_eq!(alloc.bytes_current, 0);
}
