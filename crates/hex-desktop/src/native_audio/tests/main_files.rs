use super::super::{NativeAudioSink, main_files::NativeMainFileJob};
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation,
    sampler_host_fields::{BodyRef, FileJobFields, FileSlotsFields, VecRef},
};
use hex_session::service::MainControlFileJob;
#[test]
fn actual_wrapped_file_vec_spare_bodies_exact_both_caps_and_foreign_file_refusal() {
    let dir = super::Files::new();
    let (events, _receiver) = std::sync::mpsc::sync_channel(4);
    let mut sink = NativeAudioSink::new(events);
    sink.main_files.try_reserve_exact(4).unwrap();
    for name in ["one.wav", "two.wav"] {
        sink.main_files.push(NativeMainFileJob {
            file: MainControlFileJob::create_new_on_control(&dir.0.join(name), 44_100, 513)
                .unwrap(),
            continuation: None,
        });
    }
    let fields = sink.main_file_slots_on_control();
    assert!(sink.main_files.capacity() > sink.main_files.len());
    let base = AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 16,
        root_rows: 8,
        max_retained_bytes: 1 << 20,
        max_construction_peak_bytes: 1 << 20,
    };
    let mut quote = AudioRootStorageLedger::try_new_on_control(base).unwrap();
    quote
        .borrow_sampler_file_slots_fields_on_control(&fields)
        .unwrap();
    let summary = quote.summary().unwrap();
    let exact = AudioStorageLedgerReservation {
        max_retained_bytes: summary.retained_bytes,
        max_construction_peak_bytes: summary.construction_peak_bytes,
        ..base
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    ledger
        .borrow_sampler_file_slots_fields_on_control(&fields)
        .unwrap();
    assert_eq!(ledger.summary().unwrap(), summary);
    for limits in [
        AudioStorageLedgerReservation {
            max_retained_bytes: exact.max_retained_bytes - 1,
            ..exact
        },
        AudioStorageLedgerReservation {
            max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
            ..exact
        },
    ] {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
        assert_eq!(
            ledger.borrow_sampler_file_slots_fields_on_control(&fields),
            Err(AudioStorageError::ReservationExceeded)
        );
    }
    struct Foreign<'a> {
        actual: &'a Vec<NativeMainFileJob>,
        foreign: &'a MainControlFileJob,
    }
    impl<'a> FileSlotsFields<'a> for Foreign<'a> {
        fn borrowed_frame(&self) -> BodyRef<'_> {
            BodyRef::of(self)
        }
        fn slots(&self) -> VecRef<'a> {
            VecRef::of(self.actual)
        }
        fn file(&self, _: usize) -> Result<FileJobFields<'a>, AudioStorageError> {
            self.foreign.sampler_file_fields_on_control()
        }
    }
    let foreign =
        MainControlFileJob::create_new_on_control(&dir.0.join("foreign.wav"), 44_100, 513).unwrap();
    let fields = Foreign {
        actual: &sink.main_files,
        foreign: &foreign,
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(base).unwrap();
    assert_eq!(
        ledger.borrow_sampler_file_slots_fields_on_control(&fields),
        Err(AudioStorageError::SourceMismatch)
    );
}
