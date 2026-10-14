//! Exact-runtime control disposal. This fact proves no role/catalog retirement.
use super::{ClaimIdentity, ClaimedPlaybackOwner};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlTeardownError {
    Incomplete,
    Consumed,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Owned,
    Pending,
    Complete,
}
/// Persistent control-only capsule. Keep it anchored after an incomplete result;
/// dropping an incomplete capsule is not a recovery operation. No raw owner or
/// callback rendering/mutation is exposed.
pub struct ControlPlaybackTeardown {
    owner: Option<ClaimedPlaybackOwner>,
    identity: ClaimIdentity,
    phase: Phase,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    proof_taken: bool,
    fixture_incomplete: bool,
}
/// Move-only actual destructor-return fact. Private construction, no Clone or
/// role/catalog/admission assertion. Audio must additionally bind its own custody.
pub struct RuntimeDisposed {
    identity: ClaimIdentity,
    fixture_incomplete: bool,
}
impl RuntimeDisposed {
    pub fn is_incomplete_fixture(&self) -> bool {
        self.fixture_incomplete
    }
    pub fn identity(&self) -> ClaimIdentity {
        self.identity
    }
}
impl ClaimedPlaybackOwner {
    pub fn into_control_teardown(self) -> ControlPlaybackTeardown {
        ControlPlaybackTeardown {
            identity: self.identity(),
            fixture_incomplete: self.fixture_incomplete,
            owner: Some(self),
            phase: Phase::Owned,
            first_panic: None,
            proof_taken: false,
        }
    }
}
impl ControlPlaybackTeardown {
    pub fn identity(&self) -> ClaimIdentity {
        self.identity
    }
    /// Borrow this capsule from persistent control storage OUTSIDE all ordinary
    /// root locks. Pending precedes owner take. Never retries failed destruction.
    pub fn dispose_on_control(&mut self) -> Result<RuntimeDisposed, ControlTeardownError> {
        if self.phase == Phase::Pending {
            return Err(ControlTeardownError::Incomplete);
        }
        if self.proof_taken {
            return Err(ControlTeardownError::Consumed);
        }
        if self.phase == Phase::Owned {
            if self.owner.is_none() {
                self.phase = Phase::Pending;
                return Err(ControlTeardownError::Incomplete);
            }
            self.phase = Phase::Pending;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                drop(self.owner.take());
            }));
            if let Err(payload) = result {
                self.first_panic = Some(payload); // first attempt, prevalidated empty
                return Err(ControlTeardownError::Incomplete);
            }
            self.phase = Phase::Complete;
        }
        self.proof_taken = true;
        Ok(RuntimeDisposed {
            identity: self.identity,
            fixture_incomplete: self.fixture_incomplete,
        })
    }
}

#[cfg(feature = "instrument-test-support")]
pub(super) struct DisposalProbe {
    address: usize,
    operation: Box<dyn FnMut(usize) + Send>,
}
#[cfg(feature = "instrument-test-support")]
impl Drop for DisposalProbe {
    fn drop(&mut self) {
        (self.operation)(self.address);
    }
}
#[cfg(feature = "instrument-test-support")]
impl ClaimedPlaybackOwner {
    /// Instrumented actual-owner field; absent from ordinary builds. Called
    /// after the exact original Playback Box field is destroyed on this thread.
    #[doc(hidden)]
    pub fn observe_disposal_for_test(&mut self, operation: impl FnMut(usize) + Send + 'static) {
        self.disposal_probe = Some(Box::new(DisposalProbe {
            address: self.playback.as_ref() as *const crate::playback::Playback as usize,
            operation: Box::new(operation),
        }));
    }
}

/// Pairing/destruction fixture ONLY. Real owning compile ingress, but ledger
/// coverage is intentionally INCOMPLETE. Never passes normal report/admission.
#[cfg(feature = "instrument-test-support")]
#[doc(hidden)]
pub fn incomplete_owned_pair_for_test(
    revision: crate::OwnedPreparedRevision,
) -> Result<
    (ClaimedPlaybackOwner, super::AnchoredPlaybackClaims),
    (crate::SessionPlaybackError, crate::OwnedPreparedRevision),
> {
    let mut playback =
        Box::new(crate::playback::Playback::try_from_owned_prepared_with_session(revision)?);
    playback.mark_incomplete_claim_fixture();
    let identity = super::ClaimIdentity(
        playback
            .claim_runtime_identity()
            .expect("Session fixture runtime"),
    );
    let ledger = super::Ledger {
        items: Vec::new(),
        anchors: vec![super::Anchor::Revision(std::sync::Arc::clone(
            playback.claim_prepared_owner().expect("owned fixture root"),
        ))],
    };
    Ok((
        ClaimedPlaybackOwner {
            playback,
            identity,
            fixture_incomplete: true,
            disposal_probe: None,
        },
        super::AnchoredPlaybackClaims {
            ledger,
            identity,
            fixture_incomplete: true,
            report: super::ClaimCapacityReport::default(),
        },
    ))
}

#[cfg(all(test, feature = "instrument-test-support"))]
mod tests {
    use super::*;
    use crate::storage_report::{ClaimError, ClaimReservation, PlaybackClaims};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering::Relaxed},
    };
    fn prepared() -> crate::OwnedPreparedRevision {
        crate::PreparedRevision::prepare(
            &hex_project::Project::new(48_000).unwrap(),
            &std::collections::BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared()
    }
    #[test]
    fn incomplete_origin_survives_actual_disposal_and_normal_admission_still_refuses() {
        let root = prepared();
        let alias = root.clone_on_control();
        let (owner, anchors) = incomplete_owned_pair_for_test(root).ok().unwrap();
        let recovered = anchors.reunite_on_control(owner).ok().unwrap();
        assert!(recovered.is_incomplete_fixture());
        let original = recovered.into_playback_on_control();
        let address = original.as_ref() as *const crate::playback::Playback;
        let refusal = PlaybackClaims::try_prepare_on_control(
            original,
            ClaimReservation {
                items: 4096,
                anchors: 64,
                max_prepared_data_bytes: usize::MAX,
                max_construction_data_bytes: usize::MAX,
                other_retained_bytes: 0,
                other_construction_bytes: 0,
            },
        )
        .err()
        .unwrap();
        assert_eq!(refusal.reason, ClaimError::Unknown);
        assert_eq!(
            refusal.playback.as_ref() as *const crate::playback::Playback,
            address
        );
        assert!(refusal.report.graph_data_bytes.is_none());
        drop(refusal);
        let (mut owner, anchors) = incomplete_owned_pair_for_test(alias).ok().unwrap();
        let identity = owner.identity();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let drops = Arc::clone(&observed);
        owner.observe_disposal_for_test(move |address| {
            drops
                .lock()
                .unwrap()
                .push((address, std::thread::current().id()))
        });
        let mut capsule = owner.into_control_teardown();
        let proof = capsule.dispose_on_control().unwrap();
        assert_eq!(proof.identity(), identity);
        assert!(proof.is_incomplete_fixture());
        assert!(anchors.is_incomplete_fixture());
        assert!(anchors.report().graph_data_bytes.is_none());
        assert_eq!(observed.lock().unwrap().len(), 1);
        assert_eq!(observed.lock().unwrap()[0].1, std::thread::current().id());
        assert!(matches!(
            capsule.dispose_on_control(),
            Err(ControlTeardownError::Consumed)
        ));
        assert_eq!(observed.lock().unwrap().len(), 1);
    }
    #[test]
    fn actual_disposal_field_panic_is_a_persistent_tombstone_with_first_payload() {
        let (mut owner, anchors) = incomplete_owned_pair_for_test(prepared()).ok().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let drops = Arc::clone(&counter);
        owner.observe_disposal_for_test(move |_| {
            drops.fetch_add(1, Relaxed);
            panic!("sealed owner field destructor");
        });
        let mut capsule = owner.into_control_teardown();
        assert!(matches!(
            capsule.dispose_on_control(),
            Err(ControlTeardownError::Incomplete)
        ));
        assert!(capsule.owner.is_none());
        let payload = capsule.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any
            as *const () as usize;
        assert!(anchors.is_incomplete_fixture());
        assert!(anchors.report().graph_data_bytes.is_none());
        assert!(matches!(
            capsule.dispose_on_control(),
            Err(ControlTeardownError::Incomplete)
        ));
        assert_eq!(
            capsule.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any as *const ()
                as usize,
            payload
        );
        assert_eq!(counter.load(Relaxed), 1);
    }
}
