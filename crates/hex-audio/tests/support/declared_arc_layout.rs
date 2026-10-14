//! Private test-only declaration for audited Rust alloc; not admission evidence.
//! No product API/build gate is added. Callers supply the compiler selected by
//! the test runner; unsupported observations refuse before constructor use.
use std::{
    alloc::Layout,
    sync::{Arc, Weak, atomic::AtomicUsize},
};
const COMMIT: &str = "4a4ef493e3a1488c6e321570238084b38948f6db";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Compiler,
    Target,
    LayoutOverflow,
    UntrackedOrigin,
}
#[derive(Debug)]
pub struct PassedCompilerTargetProof {
    target: &'static str,
}
impl PassedCompilerTargetProof {
    pub fn from_runner_observation(verbose: &str, running_target: &str) -> Result<Self, Refusal> {
        if !verbose.lines().any(|line| line == "release: 1.94.0")
            || !verbose
                .lines()
                .any(|line| line == format!("commit-hash: {COMMIT}"))
        {
            return Err(Refusal::Compiler);
        }
        if !matches!(
            running_target,
            "x86_64-unknown-linux-gnu" | "aarch64-apple-darwin"
        ) || !verbose
            .lines()
            .any(|line| line == format!("host: {running_target}"))
        {
            return Err(Refusal::Target);
        }
        Ok(Self {
            target: match running_target {
                "x86_64-unknown-linux-gnu" => "x86_64-unknown-linux-gnu",
                "aarch64-apple-darwin" => "aarch64-apple-darwin",
                _ => return Err(Refusal::Target),
            },
        })
    }
    pub fn target(&self) -> &str {
        self.target
    }
}
#[derive(Debug, Clone, Copy)]
pub struct DeclaredRequestedLayout {
    payload: Layout,
    requested: Layout,
    target: &'static str,
}
impl DeclaredRequestedLayout {
    pub fn for_payload(
        proof: &PassedCompilerTargetProof,
        payload: Layout,
    ) -> Result<Self, Refusal> {
        let requested = Layout::array::<AtomicUsize>(2)
            .map_err(|_| Refusal::LayoutOverflow)?
            .extend(payload)
            .map_err(|_| Refusal::LayoutOverflow)?
            .0
            .pad_to_align();
        Ok(Self {
            payload,
            requested,
            target: proof.target,
        })
    }
    pub fn sized<T>(proof: &PassedCompilerTargetProof) -> Result<Self, Refusal> {
        Self::for_payload(proof, Layout::new::<T>())
    }
    pub fn slice<T>(proof: &PassedCompilerTargetProof, len: usize) -> Result<Self, Refusal> {
        Self::for_payload(
            proof,
            Layout::array::<T>(len).map_err(|_| Refusal::LayoutOverflow)?,
        )
    }
    pub fn source_commit(self) -> &'static str {
        COMMIT
    }
    pub fn target(self) -> &'static str {
        self.target
    }
    pub fn requested(self) -> Layout {
        self.requested
    }
    pub fn payload(self) -> Layout {
        self.payload
    }
    /// Existing counter cannot observe requested alignment. Never promote this
    /// declaration to completeness based on requested-size tests alone.
    pub fn requested_alignment_verified(self) -> bool {
        false
    }
}
/// Only these known allocating constructor paths can capture a declaration.
/// No raw/custom-allocator/default-empty constructor or public origin assertion.
pub struct TrackedArc<T: ?Sized> {
    owner: Arc<T>,
    declaration: DeclaredRequestedLayout,
}
impl<T> TrackedArc<T> {
    pub fn new(proof: &PassedCompilerTargetProof, value: T) -> Result<Self, Refusal> {
        let declaration = DeclaredRequestedLayout::sized::<T>(proof)?;
        Ok(Self {
            owner: Arc::new(value),
            declaration,
        })
    }
}
impl<T> TrackedArc<[T]> {
    pub fn from_vec(proof: &PassedCompilerTargetProof, value: Vec<T>) -> Result<Self, Refusal> {
        let declaration = DeclaredRequestedLayout::slice::<T>(proof, value.len())?;
        Ok(Self {
            owner: Arc::from(value),
            declaration,
        })
    }
}
impl TrackedArc<str> {
    pub fn from_str(proof: &PassedCompilerTargetProof, value: &str) -> Result<Self, Refusal> {
        let declaration = DeclaredRequestedLayout::slice::<u8>(proof, value.len())?;
        Ok(Self {
            owner: Arc::from(value),
            declaration,
        })
    }
}
impl<T: ?Sized> TrackedArc<T> {
    pub fn owner(&self) -> &Arc<T> {
        &self.owner
    }
    pub fn declaration(&self) -> DeclaredRequestedLayout {
        self.declaration
    }
    pub fn clone_tracked(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
            declaration: self.declaration,
        }
    }
    pub fn downgrade(&self) -> TrackedWeak<T> {
        TrackedWeak {
            owner: Arc::downgrade(&self.owner),
            declaration: Some(self.declaration),
        }
    }
}
pub struct TrackedWeak<T: ?Sized> {
    owner: Weak<T>,
    declaration: Option<DeclaredRequestedLayout>,
}
impl<T> TrackedWeak<T> {
    pub fn empty() -> Self {
        Self {
            owner: Weak::new(),
            declaration: None,
        }
    }
}
impl<T: ?Sized> TrackedWeak<T> {
    pub fn declaration(&self) -> Option<DeclaredRequestedLayout> {
        self.declaration
    }
    pub fn upgrade(&self) -> Option<TrackedArc<T>> {
        self.owner.upgrade().map(|owner| TrackedArc {
            owner,
            declaration: self.declaration.expect("tracked weak owner"),
        })
    }
}
pub fn untracked_origin<T: ?Sized>(_: &Arc<T>) -> Result<DeclaredRequestedLayout, Refusal> {
    Err(Refusal::UntrackedOrigin)
}
