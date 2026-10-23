//! One shared processing fence for graph facts and copied native endpoints.
//! This is the existing transport binding, not a second source-ID namespace.

/// Exact output root, canonical device instance, prepared source graph and
/// processing epoch. A copied value is an equality fence, never source custody
/// or publication authority. Endpoints remain immutable; discontinuity requires
/// fresh queues and the native owner's actual quiescence/resume acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForeignProcessorBinding {
    pub root: u64,
    pub instance: u64,
    pub graph: u64,
    pub epoch: u64,
}
impl ForeignProcessorBinding {
    pub fn is_valid(self) -> bool {
        ![self.root, self.instance, self.graph, self.epoch].contains(&0)
    }
}
