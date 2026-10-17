use super::super::*;
use super::layout::{Bank, Clock, EnableBinding, Key, Receipt, Record};

/// Owns the real legacy analytic provider; no new production capability.
pub(super) struct Root {
    pub key: Key,
    pub rate: u32,
    pub context: Box<dyn PreparedSharedAnalyticContext>,
}
impl Root {
    pub fn new(key: Key, state: &NativeDeviceState, rate: u32) -> Self {
        assert!(key.valid());
        let context = OPERATOR_METADATA
            .prepare_instrument(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: rate,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .prepare_shared_analytic_context()
            .unwrap()
            .unwrap();
        Self { key, rate, context }
    }
    pub fn operator(&self) -> &PreparedOperatorContext {
        self.context
            .analytic_continuation()
            .unwrap()
            .as_any()
            .downcast_ref()
            .unwrap()
    }
    pub fn configuration(&self) -> u64 {
        self.operator().configuration.configuration_id()
    }
    pub fn endpoint_bank(&self) -> Bank {
        let settings = OperatorSettings::from_state(&self.operator().authored_state).unwrap();
        let mut bank = Bank::default();
        for (weights, waveform) in bank.builtin_weights.iter_mut().zip(settings.waveforms) {
            weights[waveform.choice() as usize] = 1.0;
        }
        bank
    }
    pub fn levels(&self, binding: EnableBinding, at: u64) -> Option<[f64; 4]> {
        if binding.law != self.key {
            return None;
        }
        let provider = self.context.analytic_continuation()?;
        let frame = match binding.transition {
            Some(transition) => provider
                .parameters_at(self.key.id(), &transition, at)
                .ok()?,
            None => provider.endpoint(self.key.id()),
        };
        // The existing provider produced/validated this exact target frame.
        Some(std::array::from_fn(|i| f64::from_bits(frame.words()[i])))
    }
    /// Pure builtin endpoint uses the unchanged complete legacy sample route.
    pub fn builtin_endpoint(
        &self,
        bank: Bank,
        note: &PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
    ) -> Option<StereoFrame> {
        if bank != self.endpoint_bank() {
            return None;
        }
        self.context
            .analytic_continuation()?
            .validate_note(note)
            .ok()?;
        Some(self.context.sample(note, age, gate))
    }
    pub fn compatible(&self, target: &Self) -> bool {
        let a = self.operator();
        let b = target.operator();
        let x = a.authored_state.payload();
        let y = b.authored_state.payload();
        self.rate == target.rate
            && a.authored_state.state_version() == OPERATOR_STATE_VERSION
            && b.authored_state.state_version() == OPERATOR_STATE_VERSION
            && x.len() == PAYLOAD_BYTES
            && y.len() == PAYLOAD_BYTES
            && x[..224] == y[..224]
            && x[240..364] == y[240..364]
            && x[380..] == y[380..]
            && a.continuation_format == b.continuation_format
    }
}

#[derive(Clone, Copy)]
pub(super) struct Curve {
    pub start: Bank,
    pub target: Bank,
    pub clock: Clock,
}
impl Curve {
    pub fn at(self, frame: u64) -> Option<Bank> {
        self.start
            .interpolate(self.target, self.clock.amount(frame)?)
    }
}
pub(super) struct CaptureRequest {
    pub enable: EnableBinding,
    pub curve: Curve,
    pub frame: u64,
    pub clock_generation: u64,
    pub recipe: u64,
}
pub(super) fn capture(
    source: &Root,
    target: &Root,
    enable_root: &Root,
    request: CaptureRequest,
) -> Option<Record> {
    let CaptureRequest {
        enable,
        curve,
        frame,
        clock_generation,
        recipe,
    } = request;
    if !source.compatible(target)
        || !source.compatible(enable_root)
        || clock_generation == 0
        || recipe == 0
    {
        return None;
    }
    Some(Record {
        bank: curve.at(frame)?,
        levels: enable_root.levels(enable, frame)?,
        receipt: Receipt {
            source: source.key,
            target: target.key,
            clock_generation,
            boundary: frame,
            source_configuration: source.configuration(),
            target_configuration: target.configuration(),
        },
        harmonic_clock: Clock {
            start: frame,
            frames: 240,
            reserved: 0,
        },
        enable,
        recipe,
        format: 1,
        reserved: 0,
    })
}
