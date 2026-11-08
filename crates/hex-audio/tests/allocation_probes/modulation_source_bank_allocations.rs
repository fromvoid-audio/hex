//! Isolated source-bank transactions, not native callback integration.
use hex_lattice::{ModulationLfoBank, ModulationSourceId, PreparedLfoSource};
use std::hint::black_box;

#[test]
fn source_bank_render_commit_rollback_and_inherit_do_not_allocate() {
    let id = ModulationSourceId::new(1).unwrap();
    let source = PreparedLfoSource::free(id, 48_000.0, 7.0, 0.0).unwrap();
    let mut first = ModulationLfoBank::new(&[source], 128).unwrap();
    let mut second = ModulationLfoBank::new(&[source], 128).unwrap();
    let info = allocation_counter::measure(|| {
        for _ in 0..32 {
            first.begin();
            first.render_chunk(128).unwrap();
            black_box(first.samples(id).unwrap());
            first.rollback();
            first.begin();
            first.render_chunk(128).unwrap();
            first.commit().unwrap();
            second.inherit_committed(&first).unwrap();
        }
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
    assert_eq!(info.count_current, 0);
    assert_eq!(info.bytes_current, 0);
}
