//! Only the existing Project analyzer packet constructor. Each record stays in
//! the existing NativeBank; its affine lease follows the SAME Box through the
//! two queues and pending/callback fields. Neither sizes nor addresses adopt a
//! legacy owner. The two actual allocations remain DATA until normal drop.
use super::*;

/// ```compile_fail
/// fn escape(value: &Box<[[f32; 2]; 8]>) -> hex_engine::sampler_host_fields::NativeSpectrumPacketFields<'static> {
///     hex_engine::sampler_host_fields::NativeSpectrumPacketFields::of(hex_engine::sampler_host_fields::BodyRef::of(value), value)
/// }
/// ```
pub struct NativeSpectrumPacketFields<'a> {
    body: BodyRef<'a>,
    sample_field: BodyRef<'a>,
    samples: BodyRef<'a>,
}
impl<'a> NativeSpectrumPacketFields<'a> {
    #[expect(
        clippy::borrowed_box,
        reason = "the actual owning field must be contained in the packet"
    )]
    pub fn of<const N: usize>(body: BodyRef<'a>, samples: &'a Box<[[f32; 2]; N]>) -> Self {
        Self {
            body,
            sample_field: BodyRef::of(samples),
            samples: BodyRef::of(samples.as_ref()),
        }
    }
    fn dimensions(&self) -> Result<[usize; 2], AudioStorageError> {
        if !self.body.contains(self.sample_field) || self.samples.layout_bytes() == 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok([self.body.layout_bytes(), self.samples.layout_bytes()])
    }
}
struct PacketStorage {
    bytes: [usize; 2],
    lineage: u64,
    phase: AtomicU8,
}
pub struct NativeSpectrumPacketRegistration {
    state: SharedMetadata<PacketStorage>,
}
/// ```compile_fail
/// fn duplicate(value: hex_engine::sampler_host_fields::NativeSpectrumPacketLease) { let _ = value.clone(); }
/// ```
pub struct NativeSpectrumPacketLease {
    state: SharedMetadata<PacketStorage>,
}
impl NativeSpectrumPacketRegistration {
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<PacketStorage>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<PacketStorage>()
            .checked_add(size_of::<(Self, NativeSpectrumPacketLease)>())
            .and_then(|n| n.checked_add(size_of::<NativeSpectrumPacketFields<'_>>()))
            .ok_or(AudioStorageError::Overflow)
    }
    /// Sole audited producer is the actual fresh private Audio packet, before
    /// escape and after the existing combined cohort preflight.
    pub fn pair_actual_packet_on_control(
        fields: NativeSpectrumPacketFields<'_>,
    ) -> Result<(Self, NativeSpectrumPacketLease), AudioStorageError> {
        Self::metadata_requested_bytes_on_control()?;
        let bytes = fields.dimensions()?;
        bytes[0]
            .checked_add(bytes[1])
            .ok_or(AudioStorageError::Overflow)?;
        let lineage = crate::audio_only_inputs::handle()?;
        let state = SharedMetadata::new_on_control(PacketStorage {
            bytes,
            lineage,
            phase: AtomicU8::new(LIVE),
        });
        let lease = NativeSpectrumPacketLease {
            state: state.clone(),
        };
        Ok((Self { state }, lease))
    }
    pub fn matches_actual_lease_on_control(&self, lease: &NativeSpectrumPacketLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub fn matches_actual_packet_on_control(
        &self,
        lease: &NativeSpectrumPacketLease,
        fields: NativeSpectrumPacketFields<'_>,
    ) -> bool {
        self.matches_actual_lease_on_control(lease)
            && matches!(fields.dimensions(), Ok(bytes) if bytes == self.state.bytes)
    }
    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state.phase.load(Ordering::Acquire) == DISPOSED
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
        if phase == LIVE {
            let mut physical = Owner::exclusive(self.state.as_ref());
            physical.domain = OwnerDomain::RetainedNative;
            for (index, bytes) in self.state.bytes.iter().enumerate() {
                ledger.bytes(
                    physical,
                    Location("native-spectrum-packet-allocation", index),
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
                Location("native-spectrum-packet-lease", 0),
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
impl NativeSpectrumPacketLease {
    pub fn record_actual_packet_failure_on_control(&self) {
        self.state.phase.store(FAILED, Ordering::Release);
    }
    pub fn begin_actual_packet_drop_on_control(
        &self,
        fields: NativeSpectrumPacketFields<'_>,
    ) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != LIVE
            || !matches!(fields.dimensions(), Ok(bytes) if bytes == self.state.bytes)
        {
            self.record_actual_packet_failure_on_control();
            return Err(AudioStorageError::SourceMismatch);
        }
        self.state.phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }
    pub fn complete_actual_packet_drop_on_control(&self) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != DROP_PENDING {
            self.record_actual_packet_failure_on_control();
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeSpectrumPacketLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != DISPOSED {
            self.record_actual_packet_failure_on_control();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Packet {
        samples: Box<[[f32; 2]; 8]>,
    }
    fn fields(packet: &Packet) -> NativeSpectrumPacketFields<'_> {
        NativeSpectrumPacketFields::of(BodyRef::of(packet), &packet.samples)
    }
    fn facts(record: &NativeSpectrumPacketRegistration) -> Result<usize, AudioStorageError> {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 16,
                handle_rows: 16,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            })?;
        record.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok(ledger.summary()?.data_bytes)
    }
    #[test]
    fn foreign_sample_field_refuses_before_metadata_birth_without_allocation() {
        let first = Packet {
            samples: Box::new([[0.0; 2]; 8]),
        };
        let other = Packet {
            samples: Box::new([[0.0; 2]; 8]),
        };
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                NativeSpectrumPacketRegistration::pair_actual_packet_on_control(
                    NativeSpectrumPacketFields::of(BodyRef::of(&first), &other.samples)
                ),
                Err(AudioStorageError::SourceMismatch)
            ));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[test]
    fn actual_metadata_alias_omission_fails_closure_and_normal_box_disposal_keeps_only_metadata() {
        let packet = Box::new(Packet {
            samples: Box::new([[0.0; 2]; 8]),
        });
        let (record, lease) =
            NativeSpectrumPacketRegistration::pair_actual_packet_on_control(fields(&packet))
                .unwrap();
        let metadata =
            NativeSpectrumPacketRegistration::metadata_requested_bytes_on_control().unwrap();
        assert_eq!(
            facts(&record).unwrap(),
            metadata + size_of::<Packet>() + size_of::<[[f32; 2]; 8]>()
        );
        let omitted = lease.state.clone();
        assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
        drop(omitted);
        assert!(facts(&record).is_ok());
        lease
            .begin_actual_packet_drop_on_control(fields(&packet))
            .unwrap();
        assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
        drop(packet);
        lease.complete_actual_packet_drop_on_control().unwrap();
        drop(lease);
        assert!(record.is_normally_disposed_on_control());
        assert_eq!(facts(&record).unwrap(), metadata);
    }
    #[test]
    fn foreign_or_repeated_destructive_shape_stays_failed_without_disposal() {
        let packet = Packet {
            samples: Box::new([[0.0; 2]; 8]),
        };
        let foreign = Packet {
            samples: Box::new([[0.0; 2]; 8]),
        };
        let (record, lease) =
            NativeSpectrumPacketRegistration::pair_actual_packet_on_control(fields(&packet))
                .unwrap();
        assert!(
            lease
                .begin_actual_packet_drop_on_control(NativeSpectrumPacketFields::of(
                    BodyRef::of(&packet),
                    &foreign.samples
                ))
                .is_err()
        );
        assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
        assert!(lease.complete_actual_packet_drop_on_control().is_err());
        assert!(!record.is_normally_disposed_on_control());
    }
}
