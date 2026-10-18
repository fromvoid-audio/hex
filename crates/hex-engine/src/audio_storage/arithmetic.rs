//! Private allocation-free traversal, never a normal inventory or admission.
//! All ordinary DATA validators run. Membership gates cannot prune children:
//! arithmetic buffers are deliberately absent, rather than fabricated records.
use super::*;

#[derive(Clone, Copy)]
enum Observation<'a> {
    Data(Owner<'a>, Location, Declaration),
    Existing(Owner<'a>),
}
#[derive(Clone, Copy)]
enum Pass<'a> {
    Count,
    Capture { index: usize },
    Prefix { index: usize, key: Observation<'a> },
}
pub(super) struct Arithmetic<'a> {
    pass: Pass<'a>,
    observations: usize,
    bytes: usize,
    handles: usize,
    roots: usize,
    existing: bool,
    captured: Option<Observation<'a>>,
    matched: bool,
}
impl<'a> Arithmetic<'a> {
    fn new(pass: Pass<'a>) -> Self {
        Self {
            pass,
            observations: 0,
            bytes: 0,
            handles: 0,
            roots: 0,
            existing: false,
            captured: None,
            matched: false,
        }
    }
    fn observe(&mut self, event: Observation<'a>) -> Result<(), AudioStorageError> {
        let index = self.observations;
        self.observations = index.checked_add(1).ok_or(AudioStorageError::Overflow)?;
        match self.pass {
            Pass::Capture { index: selected } if index == selected => self.captured = Some(event),
            Pass::Prefix {
                index: selected,
                key,
            } if index < selected => {
                if let Observation::Data(owner, location, declaration) = event {
                    match key {
                        Observation::Data(target, at, declared)
                            if target.same(owner) && at == location =>
                        {
                            if declared != declaration {
                                return Err(AudioStorageError::UnknownStorage);
                            }
                            self.matched = true;
                        }
                        Observation::Existing(target) if target.same(owner) => self.matched = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn data(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        declaration: Declaration,
    ) -> Result<(), AudioStorageError> {
        self.bytes = self
            .bytes
            .checked_add(declaration.bytes)
            .ok_or(AudioStorageError::Overflow)?;
        self.observe(Observation::Data(owner, location, declaration))
    }
    pub(super) fn existing(&mut self, owner: Owner<'a>) -> Result<(), AudioStorageError> {
        self.existing = true;
        self.observe(Observation::Existing(owner))
    }
    pub(super) fn handle(&mut self) -> Result<(), AudioStorageError> {
        self.handles = self
            .handles
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
    pub(super) fn root(&mut self) -> Result<(), AudioStorageError> {
        self.roots = self
            .roots
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
}
// Arithmetic only. No public summary/closure methods and no owner proof. The
// closed compiler operation must add BOTH live ledger bodies/backings, actual
// guarded growth and construction overlap before allocating the normal ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArithmeticCost {
    pub(crate) data_bytes: usize,
    pub(crate) growth_bytes: usize,
    pub(crate) replayed: bool,
    pub(crate) observation_visits: usize,
}
impl<'a> AudioRootStorageLedger<'a> {
    fn arithmetic_ledger(reservation: AudioStorageLedgerReservation) -> Self {
        Self {
            data: Vec::new(),
            handles: Vec::new(),
            roots: Vec::new(),
            reservation,
            growth: 0,
            unknown: 0,
            valid: true,
            arithmetic: Some(Arithmetic::new(Pass::Count)),
        }
    }
    fn run_arithmetic_pass(
        &mut self,
        pass: Pass<'a>,
        visit: &impl Fn(&mut Self) -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        self.arithmetic = Some(Arithmetic::new(pass));
        self.growth = 0;
        self.unknown = 0;
        self.valid = true;
        visit(self)?;
        self.arithmetic_completion()
    }
    fn arithmetic_shape(
        &self,
    ) -> Result<(usize, usize, usize, usize, bool, usize), AudioStorageError> {
        let pass = self
            .arithmetic
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        Ok((
            pass.observations,
            pass.bytes,
            pass.handles,
            pass.roots,
            pass.existing,
            self.growth,
        ))
    }
    pub(super) fn arithmetic_completion(&self) -> Result<(), AudioStorageError> {
        if !self.valid || self.unknown != 0 {
            Err(AudioStorageError::UnknownStorage)
        } else {
            Ok(())
        }
    }
    // Data-only conformance wrapper. The closed compiler operation uses the
    // retained AND peak adapter below with the same mandatory leaf validators;
    // neither path exports an admission or caller-approved totals API.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "data-only conformance wrapper; production uses both ceiling limits"
        )
    )]
    pub(super) fn arithmetic_cost(
        reservation: AudioStorageLedgerReservation,
        max_data_bytes: usize,
        visit: impl Fn(&mut Self) -> Result<(), AudioStorageError>,
    ) -> Result<ArithmeticCost, AudioStorageError> {
        Self::arithmetic_cost_for_limits(reservation, max_data_bytes, usize::MAX, visit)
    }
    pub(crate) fn arithmetic_cost_for_limits(
        reservation: AudioStorageLedgerReservation,
        max_data_bytes: usize,
        max_peak_data_bytes: usize,
        visit: impl Fn(&mut Self) -> Result<(), AudioStorageError>,
    ) -> Result<ArithmeticCost, AudioStorageError> {
        let mut ledger = Self::arithmetic_ledger(reservation);
        ledger.run_arithmetic_pass(Pass::Count, &visit)?;
        let shape = ledger.arithmetic_shape()?;
        let (observations, conservative_bytes, _, _, existing, growth) = shape;
        if conservative_bytes <= max_data_bytes
            && conservative_bytes
                .checked_add(growth)
                .is_some_and(|n| n <= max_peak_data_bytes)
            && !existing
        {
            return Ok(ArithmeticCost {
                data_bytes: conservative_bytes,
                growth_bytes: growth,
                replayed: false,
                observation_visits: observations,
            });
        }
        // Bound quadratic fallback by actual observation visits, not row
        // reservation inflation. Refuse before any normal backing allocation.
        const MAX_REPLAY_VISITS: usize = 16 * 1024 * 1024;
        let visits = observations
            .checked_mul(2)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(observations))
            .ok_or(AudioStorageError::Overflow)?;
        if visits > MAX_REPLAY_VISITS {
            return Err(AudioStorageError::Capacity);
        }
        let mut bytes = 0usize;
        for index in 0..observations {
            ledger.run_arithmetic_pass(Pass::Capture { index }, &visit)?;
            if ledger.arithmetic_shape()? != shape {
                return Err(AudioStorageError::SourceMismatch);
            }
            let key = ledger
                .arithmetic
                .as_mut()
                .and_then(|pass| pass.captured.take())
                .ok_or(AudioStorageError::SourceMismatch)?;
            // Capture is discarded before Prefix: one ledger, one retained
            // borrowed observation key. No accumulating borrowed record list.
            ledger.run_arithmetic_pass(Pass::Prefix { index, key }, &visit)?;
            if ledger.arithmetic_shape()? != shape {
                return Err(AudioStorageError::SourceMismatch);
            }
            let matched = ledger
                .arithmetic
                .as_ref()
                .ok_or(AudioStorageError::UnknownStorage)?
                .matched;
            match key {
                Observation::Existing(_) if !matched => {
                    return Err(AudioStorageError::SourceMismatch);
                }
                Observation::Data(_, _, declaration) if !matched => {
                    bytes = bytes
                        .checked_add(declaration.bytes)
                        .ok_or(AudioStorageError::Overflow)?;
                }
                _ => {}
            }
        }
        if bytes > max_data_bytes
            || bytes
                .checked_add(growth)
                .ok_or(AudioStorageError::Overflow)?
                > max_peak_data_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(ArithmeticCost {
            data_bytes: bytes,
            growth_bytes: growth,
            replayed: true,
            observation_visits: visits,
        })
    }
}

#[cfg(test)]
mod tests;

impl<'a> AudioRootStorageLedger<'a> {
    /// Revisit the actual opaque original inventory, including its still-live
    /// body and three backings. This never substitutes for the mandatory
    /// compiler input visitors. Old and freshly observed physical fields are
    /// idempotent, while inconsistent owner/field declarations refuse.
    pub(crate) fn original_ledger_storage(
        &mut self,
        original: &'a AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        original
            .summary()?
            .require_known_storage()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        self.mutate(|ledger| {
            ledger.vec(&original.data)?;
            ledger.vec(&original.handles)?;
            ledger.vec(&original.roots)?;
            // The real inline data Vec field anchors its enclosing ledger body;
            // its backing uses a separate location. No synthetic owner/ID.
            ledger.bytes(
                Owner::exclusive_vec(&original.data),
                Location("inspection-ledger-inline", 0),
                size_of_val(original),
            )?;
            for row in &original.data {
                ledger.data(row.owner, row.location, row.declaration, row.streamed)?;
            }
            for handle in &original.handles {
                if ledger.arithmetic.is_none() {
                    if let Some(existing) = ledger
                        .handles
                        .iter()
                        .find(|row| row.field.same(handle.field) && row.location == handle.location)
                    {
                        if !existing.owner.same(handle.owner) {
                            return Err(AudioStorageError::SourceMismatch);
                        }
                        continue;
                    }
                }
                ledger.handle_at(handle.owner, handle.field, handle.location, handle.id)?;
            }
            for root in &original.roots {
                if let Some(arithmetic) = &mut ledger.arithmetic {
                    arithmetic.root()?;
                } else if !ledger.roots.iter().any(|row| row.owner.same(root.owner)) {
                    if ledger.roots.len() == ledger.reservation.root_rows {
                        return Err(AudioStorageError::Capacity);
                    }
                    ledger.roots.push(Root {
                        owner: root.owner,
                        _role: root._role,
                    });
                }
            }
            Ok(())
        })
    }
}

impl AudioRootStorageLedger<'_> {
    pub(crate) fn original_live_growth_allowance(&self) -> Result<usize, AudioStorageError> {
        self.summary()?
            .require_known_storage()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        Ok(self.growth)
    }
}
