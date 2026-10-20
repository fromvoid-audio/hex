//! Five constructor-preallocated Project result buffers. Inline wrapper bodies
//! belong to their actual enclosing field/ring/frame; this record separately
//! charges ONLY the owning fixed-frame allocation plus actual lease metadata.
//! No result/copy allocation occurs after this constructor. The nonClone lease
//! moves with that same Box through the existing worker/reader/UI custody.
use super::*;

pub struct NativeSpectrumResultFields<'a> {
    body: BodyRef<'a>,
    frame_field: BodyRef<'a>,
    frames: BodyRef<'a>,
}
impl<'a> NativeSpectrumResultFields<'a> {
    /// The one audited Audio result producer exhaustively validates every
    /// inline frame. BodyRef certifies footprint only, never child absence.
    pub fn of(body: BodyRef<'a>, frame_field: BodyRef<'a>, frames: BodyRef<'a>) -> Self {
        Self {
            body,
            frame_field,
            frames,
        }
    }
    fn dimensions(&self) -> Result<[usize; 1], AudioStorageError> {
        if !self.body.contains(self.frame_field) || self.frames.layout_bytes() == 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok([self.frames.layout_bytes()])
    }
}
struct ResultStorage {
    bytes: [usize; 1],
    lineage: u64,
    home_lineage: u64,
    home_live: std::sync::atomic::AtomicBool,
    phase: AtomicU8,
}
pub struct NativeSpectrumResultRegistration {
    state: SharedMetadata<ResultStorage>,
}
/// ```compile_fail
/// fn duplicate(value: hex_engine::sampler_host_fields::NativeSpectrumResultLease) { let _ = value.clone(); }
/// ```
pub struct NativeSpectrumResultLease {
    state: SharedMetadata<ResultStorage>,
}
/// Affine constructor-home alias in the actual reader. It recognizes only the
/// paired payload lease, never a scalar pool ID or address certificate.
pub struct NativeSpectrumResultHome {
    state: SharedMetadata<ResultStorage>,
}
impl NativeSpectrumResultHome {
    pub fn matches_actual_lease_on_control(&self, lease: &NativeSpectrumResultLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.home_live.load(Ordering::Acquire)
            && self.state.phase.load(Ordering::Acquire) == LIVE
    }
}
impl Drop for NativeSpectrumResultHome {
    fn drop(&mut self) {
        self.state.home_live.store(false, Ordering::Release);
    }
}
impl NativeSpectrumResultRegistration {
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<ResultStorage>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<ResultStorage>()
            .checked_add(size_of::<(
                Self,
                NativeSpectrumResultLease,
                NativeSpectrumResultHome,
            )>())
            .and_then(|n| n.checked_add(size_of::<NativeSpectrumResultFields<'_>>()))
            .ok_or(AudioStorageError::Overflow)
    }
    /// Sole audited producer is the actual fresh private Audio result, before
    /// escape and after the existing combined cohort preflight.
    pub fn pair_actual_result_on_control(
        fields: NativeSpectrumResultFields<'_>,
    ) -> Result<(Self, NativeSpectrumResultLease, NativeSpectrumResultHome), AudioStorageError>
    {
        Self::metadata_requested_bytes_on_control()?;
        let bytes = fields.dimensions()?;
        let lineage = crate::audio_only_inputs::handle()?;
        let home_lineage = crate::audio_only_inputs::handle()?;
        let state = SharedMetadata::new_on_control(ResultStorage {
            bytes,
            lineage,
            home_lineage,
            home_live: std::sync::atomic::AtomicBool::new(true),
            phase: AtomicU8::new(LIVE),
        });
        let lease = NativeSpectrumResultLease {
            state: state.clone(),
        };
        let home = NativeSpectrumResultHome {
            state: state.clone(),
        };
        Ok((Self { state }, lease, home))
    }
    pub fn matches_actual_home_on_control(&self, home: &NativeSpectrumResultHome) -> bool {
        self.state.shares_owner_with(&home.state)
            && self.state.home_live.load(Ordering::Acquire)
            && matches!(self.state.phase.load(Ordering::Acquire), LIVE | DISPOSED)
    }
    pub fn matches_actual_lease_on_control(&self, lease: &NativeSpectrumResultLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub fn matches_actual_result_on_control(
        &self,
        lease: &NativeSpectrumResultLease,
        fields: NativeSpectrumResultFields<'_>,
    ) -> bool {
        self.matches_actual_lease_on_control(lease)
            && matches!(fields.dimensions(), Ok(bytes) if bytes == self.state.bytes)
    }
    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state.phase.load(Ordering::Acquire) == DISPOSED
            && !self.state.home_live.load(Ordering::Acquire)
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let phase = self.state.phase.load(Ordering::Acquire);
        if !matches!(phase, LIVE | DISPOSED) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.state.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.state) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        if self.state.home_live.load(Ordering::Acquire) {
            let shared = Owner::shared(paired.arc());
            if let Some(arithmetic) = &mut ledger.arithmetic {
                arithmetic.existing(shared)?;
            }
            ledger.handle_at(
                shared,
                Owner::exclusive(&self.state.home_lineage),
                Location("native-spectrum-result-home", 0),
                self.state.home_lineage,
            )?;
        }
        if phase == LIVE {
            let mut physical = Owner::exclusive(self.state.as_ref());
            physical.domain = OwnerDomain::RetainedNative;
            for (index, bytes) in self.state.bytes.iter().enumerate() {
                ledger.bytes(
                    physical,
                    Location("native-spectrum-result-allocation", index),
                    *bytes,
                )?;
            }
            let shared = Owner::shared(paired.arc());
            if let Some(arithmetic) = &mut ledger.arithmetic {
                arithmetic.existing(shared)?;
            }
            ledger.handle_at(
                shared,
                Owner::exclusive(&self.state.lineage),
                Location("native-spectrum-result-lease", 0),
                self.state.lineage,
            )?;
        }
        Ok(())
    }
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(ledger))
    }
}
impl NativeSpectrumResultLease {
    pub fn record_actual_result_failure_on_control(&self) {
        self.state.phase.store(FAILED, Ordering::Release);
    }
    pub fn begin_actual_result_drop_on_control(
        &self,
        fields: NativeSpectrumResultFields<'_>,
    ) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != LIVE
            || !matches!(fields.dimensions(), Ok(bytes) if bytes == self.state.bytes)
        {
            self.record_actual_result_failure_on_control();
            return Err(AudioStorageError::SourceMismatch);
        }
        self.state.phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }
    pub fn complete_actual_result_drop_on_control(&self) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != DROP_PENDING {
            self.record_actual_result_failure_on_control();
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeSpectrumResultLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != DISPOSED {
            self.record_actual_result_failure_on_control();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Value {
        frames: Box<[u32; 64]>,
    }
    fn fields(value: &Value) -> NativeSpectrumResultFields<'_> {
        NativeSpectrumResultFields::of(
            BodyRef::of(value),
            BodyRef::of(&value.frames),
            BodyRef::of(value.frames.as_ref()),
        )
    }
    fn facts(record: &NativeSpectrumResultRegistration) -> Result<usize, AudioStorageError> {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 8,
                handle_rows: 8,
                root_rows: 2,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            })?;
        record.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok(ledger.summary()?.data_bytes)
    }
    #[test]
    fn actual_result_home_and_payload_have_separate_affine_lifetimes_and_alias_closure() {
        let mut value = Some(Value {
            frames: Box::new([0; 64]),
        });
        let (record, lease, home) =
            NativeSpectrumResultRegistration::pair_actual_result_on_control(fields(
                value.as_ref().unwrap(),
            ))
            .unwrap();
        let metadata =
            NativeSpectrumResultRegistration::metadata_requested_bytes_on_control().unwrap();
        assert_eq!(facts(&record).unwrap(), metadata + size_of::<[u32; 64]>());
        assert!(home.matches_actual_lease_on_control(&lease));
        let extra = home.state.clone();
        assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
        drop(extra);
        lease
            .begin_actual_result_drop_on_control(fields(value.as_ref().unwrap()))
            .unwrap();
        assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
        drop(value.take());
        lease.complete_actual_result_drop_on_control().unwrap();
        drop(lease);
        assert_eq!(facts(&record).unwrap(), metadata);
        assert!(
            !record.is_normally_disposed_on_control(),
            "actual reader home remains live"
        );
        drop(home);
        assert!(record.is_normally_disposed_on_control());
        assert_eq!(facts(&record).unwrap(), metadata);
    }
    #[test]
    fn foreign_result_field_and_foreign_home_refuse_without_constructor_allocation() {
        let a = Value {
            frames: Box::new([0; 64]),
        };
        let b = Value {
            frames: Box::new([0; 64]),
        };
        let m = allocation_counter::measure(|| {
            assert!(matches!(
                NativeSpectrumResultRegistration::pair_actual_result_on_control(
                    NativeSpectrumResultFields::of(
                        BodyRef::of(&a),
                        BodyRef::of(&b.frames),
                        BodyRef::of(b.frames.as_ref())
                    )
                ),
                Err(AudioStorageError::SourceMismatch)
            ));
        });
        assert_eq!(
            (
                m.count_total,
                m.count_current,
                m.bytes_total,
                m.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let (_ra, la, ha) =
            NativeSpectrumResultRegistration::pair_actual_result_on_control(fields(&a)).unwrap();
        let (_rb, lb, hb) =
            NativeSpectrumResultRegistration::pair_actual_result_on_control(fields(&b)).unwrap();
        assert!(!ha.matches_actual_lease_on_control(&lb));
        assert!(!hb.matches_actual_lease_on_control(&la));
    }
}
