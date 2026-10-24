//! Peak-only derived control data. No PCM, bank, device scope or path is retained.
//! Charges follow backing allocations, including snapshots surviving New/Open.
use crate::SessionError;
use hex_media::waveform::Peak;
use hex_project::{AssetId, MAX_ASSETS, OriginalSourceFacts};
use std::{
    alloc::Layout,
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

/// Maximum original-source leaf resolution. At 16 bytes per stereo leaf, the
/// shared domain admits at most 256 MiB of peak backing for 4096 long sources.
/// Short sources allocate one leaf per frame instead of the maximum capacity.
pub const SOURCE_OVERVIEW_BINS: usize = 4096;
type StereoPeak = [Peak; 2];
const BIN_BYTES: usize = SOURCE_OVERVIEW_BINS * mem::size_of::<StereoPeak>();
const MAX_BIN_BYTES: usize = MAX_ASSETS * BIN_BYTES;
type Row = (AssetId, Arc<OriginalSourceOverview>);

/// Admitted live peak backing and logical live overview/table metadata only.
/// Box/Vec backing is freed before its charge returns. Metadata quotes include
/// wrappers/charges/control blocks while logically live; they exclude Arc shells
/// awaiting deallocation (including Weak shells) and allocator bookkeeping.
/// This is not a physical heap/RSS bound, nor accounting for existing ViewState
/// parents, copied handles or all application memory. Internal Arcs stay private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceOverviewUsage {
    pub bin_capacity_bytes: usize,
    pub metadata_bytes: usize,
    pub max_bin_capacity_bytes: usize,
    pub max_metadata_bytes: usize,
}
struct BudgetInner {
    bins: AtomicUsize,
    metadata: AtomicUsize,
    max_bins: usize,
    max_metadata: usize,
    #[cfg(feature = "source-overview-test-support")]
    before_release: Option<fn(SourceOverviewUsage, usize, usize)>,
}
#[derive(Clone)]
pub(crate) struct OverviewBudget(Arc<BudgetInner>);
struct Charge {
    budget: OverviewBudget,
    bins: usize,
    metadata: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        #[cfg(feature = "source-overview-test-support")]
        if let Some(observe) = self.budget.0.before_release {
            observe(self.budget.usage(), self.bins, self.metadata);
        }
        self.budget.0.bins.fetch_sub(self.bins, Ordering::AcqRel);
        self.budget
            .0
            .metadata
            .fetch_sub(self.metadata, Ordering::AcqRel);
    }
}
// The pinned Rust ArcInner is repr(C): two AtomicUsize counters, then data.
// Native allocation probes verify these quotes against actual allocation bytes.
fn shared_bytes<T>() -> Result<usize, SessionError> {
    Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .map(|(layout, _)| layout.pad_to_align().size())
        .map_err(|_| SessionError::Invalid("source overview allocation size overflow"))
}
impl OverviewBudget {
    pub(crate) fn standard() -> Result<Self, SessionError> {
        let overview_header = shared_bytes::<OriginalSourceOverview>()?;
        let table = MAX_ASSETS
            .checked_mul(mem::size_of::<Row>())
            .and_then(|n| n.checked_add(shared_bytes::<Table>().ok()?))
            .ok_or(SessionError::Invalid("source overview table size overflow"))?;
        let base = shared_bytes::<BudgetInner>()?;
        let metadata = overview_header
            .checked_mul(MAX_ASSETS)
            .and_then(|n| n.checked_add(table.checked_mul(2)?))
            .and_then(|n| n.checked_add(base))
            .ok_or(SessionError::Invalid(
                "source overview budget size overflow",
            ))?;
        Self::with_limits(MAX_BIN_BYTES, metadata)
    }
    pub(crate) fn with_limits(max_bins: usize, max_metadata: usize) -> Result<Self, SessionError> {
        let base = shared_bytes::<BudgetInner>()?;
        if max_metadata < base {
            return Err(SessionError::Invalid(
                "source overview metadata budget is full",
            ));
        }
        Ok(Self(Arc::new(BudgetInner {
            bins: AtomicUsize::new(0),
            metadata: AtomicUsize::new(base),
            max_bins,
            max_metadata,
            #[cfg(feature = "source-overview-test-support")]
            before_release: None,
        })))
    }
    fn reserve(&self, bins: usize, metadata: usize) -> Result<Charge, SessionError> {
        self.0
            .bins
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bins).filter(|v| *v <= self.0.max_bins)
            })
            .map_err(|_| {
                SessionError::Invalid(
                    "source overview peak budget is full; release retained views and retry",
                )
            })?;
        if self
            .0
            .metadata
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(metadata)
                    .filter(|v| *v <= self.0.max_metadata)
            })
            .is_err()
        {
            self.0.bins.fetch_sub(bins, Ordering::AcqRel);
            return Err(SessionError::Invalid(
                "source overview metadata budget is full; release retained views and retry",
            ));
        }
        Ok(Charge {
            budget: self.clone(),
            bins,
            metadata,
        })
    }
    pub(crate) fn usage(&self) -> SourceOverviewUsage {
        SourceOverviewUsage {
            bin_capacity_bytes: self.0.bins.load(Ordering::Acquire),
            metadata_bytes: self.0.metadata.load(Ordering::Acquire),
            max_bin_capacity_bytes: self.0.max_bins,
            max_metadata_bytes: self.0.max_metadata,
        }
    }
}

/// Normalized stereo overview in original source-frame coordinates. Bounded
/// capacity is min(source frames, 4096); actual backing remains charged until freed.
pub struct OriginalSourceOverview {
    frames: u64,
    sample_rate: u32,
    content: OriginalSourceFacts,
    count: usize,
    // The bin backing is freed before _charge returns its capacity.
    bins: Box<[StereoPeak]>,
    _charge: Charge,
}
impl OriginalSourceOverview {
    pub fn source_frames(&self) -> u64 {
        self.frames
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn content(&self) -> OriginalSourceFacts {
        self.content
    }
    pub fn peaks(&self) -> impl ExactSizeIterator<Item = &StereoPeak> {
        self.bins.iter().take(self.count)
    }
    /// Exact half-open original-frame interval for one disjoint peak bin.
    pub fn bin_range(&self, index: usize) -> Option<std::ops::Range<u64>> {
        if index >= self.count || self.count == 0 {
            return None;
        }
        let count = u64::try_from(self.count).ok()?;
        let index = u64::try_from(index).ok()?;
        let width = self.frames / count;
        let extra = self.frames % count;
        let start = index.checked_mul(width)?.checked_add(index.min(extra))?;
        let end = start
            .checked_add(width)?
            .checked_add(u64::from(index < extra))?;
        (start < end && end <= self.frames).then_some(start..end)
    }
    /// Conservative signed stereo extrema for an original-frame interval.
    /// Only intersecting cached leaves are visited; no PCM or allocation is used.
    /// A full-width pixel projection visits at most leaves + twice its columns.
    pub fn peaks_for_frames(&self, start: u64, end: u64) -> Option<StereoPeak> {
        let end = end.min(self.frames);
        if start >= end || self.count == 0 {
            return None;
        }
        let count = self.count as u64;
        let width = self.frames / count;
        let extra = self.frames % count;
        let wide = (width + 1) * extra;
        let bin_at = |frame: u64| {
            if frame < wide {
                frame / (width + 1)
            } else {
                extra + (frame - wide) / width
            }
        };
        let first = usize::try_from(bin_at(start)).ok()?;
        let last = usize::try_from(bin_at(end - 1)).ok()?;
        let bins = self.bins.get(first..=last)?;
        let mut result = *bins.first()?;
        for pair in &bins[1..] {
            for (peak, other) in result.iter_mut().zip(pair) {
                peak.min = peak.min.min(other.min);
                peak.max = peak.max.max(other.max);
            }
        }
        Some(result)
    }
    pub fn bin_count(&self) -> usize {
        self.count
    }
    pub(crate) fn prepare(
        budget: &OverviewBudget,
        frames: &[hex_engine::Frame],
        rate: u32,
        content: OriginalSourceFacts,
        cancel: &AtomicBool,
    ) -> Result<Arc<Self>, SessionError> {
        Self::prepare_cancellable(budget, frames, rate, content, |_| {
            cancel.load(Ordering::Acquire)
        })
    }
    pub(crate) fn prepare_cancellable(
        budget: &OverviewBudget,
        frames: &[hex_engine::Frame],
        rate: u32,
        content: OriginalSourceFacts,
        cancelled: impl Fn(usize) -> bool,
    ) -> Result<Arc<Self>, SessionError> {
        if frames.is_empty() || rate == 0 || cancelled(0) {
            return Err(SessionError::Invalid(
                "source overview cancelled or source is invalid",
            ));
        }
        let count = frames.len().min(SOURCE_OVERVIEW_BINS);
        let bytes = count
            .checked_mul(mem::size_of::<StereoPeak>())
            .ok_or(SessionError::Invalid("source overview peak size overflow"))?;
        let charge = budget.reserve(bytes, shared_bytes::<Self>()?)?;
        let mut bins = Vec::new();
        bins.try_reserve_exact(count)
            .map_err(|_| SessionError::Allocation)?;
        if bins.capacity() != count {
            return Err(SessionError::Invalid(
                "source overview peak capacity differs from reservation",
            ));
        }
        let zero = Peak { min: 0.0, max: 0.0 };
        bins.resize(count, [zero; 2]);
        let mut result = Arc::new(Self {
            frames: u64::try_from(frames.len())
                .map_err(|_| SessionError::Invalid("source overview extent exceeds frame range"))?,
            sample_rate: rate,
            content,
            count,
            bins: bins.into_boxed_slice(),
            _charge: charge,
        });
        let target = Arc::get_mut(&mut result).ok_or(SessionError::Invalid(
            "source overview construction is not exclusive",
        ))?;
        let width = frames.len() / count;
        let extra = frames.len() % count;
        let mut start = 0_usize;
        let mut scanned = 0_usize;
        for (index, bin) in target.bins.iter_mut().take(count).enumerate() {
            let end = start
                .checked_add(width)
                .and_then(|n| n.checked_add(usize::from(index < extra)))
                .ok_or(SessionError::Invalid("source overview interval overflow"))?;
            let source = frames
                .get(start..end)
                .filter(|f| !f.is_empty())
                .ok_or(SessionError::Invalid("source overview interval is invalid"))?;
            let first = source
                .first()
                .ok_or(SessionError::Invalid("source overview interval is empty"))?;
            *bin = first.map(|sample| Peak {
                min: sample,
                max: sample,
            });
            for frame in source {
                if scanned & 4095 == 0 && cancelled(scanned) {
                    return Err(SessionError::Invalid("source overview cancelled"));
                }
                for (peak, sample) in bin.iter_mut().zip(frame) {
                    if !sample.is_finite() {
                        return Err(SessionError::Invalid(
                            "source overview contains nonfinite pcm",
                        ));
                    }
                    peak.min = peak.min.min(*sample);
                    peak.max = peak.max.max(*sample);
                }
                scanned += 1; // At most the admitted source slice length.
            }
            start = end;
        }
        if start != frames.len() || cancelled(scanned) {
            return Err(SessionError::Invalid(
                "source overview cancelled or incomplete",
            ));
        }
        Ok(result)
    }
}
struct Table {
    // Vec before charge: its backing is freed before the metadata charge drops.
    rows: Vec<Row>,
    _charge: Charge,
}
/// One immutable shared table. Copies allocate nothing and expose borrowed
/// lookups only; neither mutable rows nor owning overview handles escape.
#[derive(Clone, Default)]
pub struct OriginalSourceOverviews {
    table: Option<Arc<Table>>,
    context: Option<hex_engine::AudioSourceContext>,
}
impl OriginalSourceOverviews {
    /// Scalar publication binding; underlying immutable rows remain shared.
    pub fn matches_context(&self, context: hex_engine::AudioSourceContext) -> bool {
        self.context == Some(context)
    }
    pub(crate) fn at_context(mut self, context: hex_engine::AudioSourceContext) -> Self {
        self.context = Some(context);
        self
    }
    pub fn get(&self, asset: AssetId) -> Option<&OriginalSourceOverview> {
        let rows = &self.table.as_ref()?.rows;
        rows.binary_search_by_key(&asset, |(id, _)| *id)
            .ok()
            .and_then(|index| rows.get(index))
            .map(|(_, overview)| overview.as_ref())
    }
    pub fn len(&self) -> usize {
        self.table.as_ref().map_or(0, |t| t.rows.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        match (&self.table, &other.table) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
    pub(crate) fn inserted(
        &self,
        budget: &OverviewBudget,
        asset: AssetId,
        overview: Arc<OriginalSourceOverview>,
    ) -> Result<Self, SessionError> {
        if !Arc::ptr_eq(&budget.0, &overview._charge.budget.0)
            || self
                .table
                .as_ref()
                .is_some_and(|table| !Arc::ptr_eq(&budget.0, &table._charge.budget.0))
        {
            return Err(SessionError::Invalid(
                "source overview budget domain changed",
            ));
        }
        if self.get(asset).is_some() {
            return Err(SessionError::Invalid("source overview asset is duplicated"));
        }
        let count = self
            .len()
            .checked_add(1)
            .filter(|n| *n <= MAX_ASSETS)
            .ok_or(SessionError::Invalid("source overview asset limit reached"))?;
        let bytes = count
            .checked_mul(mem::size_of::<Row>())
            .and_then(|n| n.checked_add(shared_bytes::<Table>().ok()?))
            .ok_or(SessionError::Invalid("source overview table size overflow"))?;
        let charge = budget.reserve(0, bytes)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_| SessionError::Allocation)?;
        if rows.capacity() != count {
            return Err(SessionError::Invalid(
                "source overview table capacity differs from reserved allocation",
            ));
        }
        if let Some(table) = &self.table {
            rows.extend(table.rows.iter().cloned());
        }
        rows.push((asset, overview));
        rows.sort_unstable_by_key(|(id, _)| *id);
        Ok(Self {
            table: Some(Arc::new(Table {
                rows,
                _charge: charge,
            })),
            context: None,
        })
    }
}

#[cfg(test)]
mod tests;

/// Nondefault peak-only fixture. It cannot construct or expose a native bank,
/// PCM owner, scope or publication capability.
#[cfg(feature = "source-overview-test-support")]
pub mod test_support {
    use super::*;
    pub struct Fixture {
        budget: OverviewBudget,
        overview: Option<Arc<OriginalSourceOverview>>,
        table: Option<OriginalSourceOverviews>,
    }
    impl Fixture {
        pub fn new(
            asset: AssetId,
            frames: &[hex_engine::Frame],
            rate: u32,
            content: OriginalSourceFacts,
            before_release: Option<fn(SourceOverviewUsage, usize, usize)>,
        ) -> Result<Self, SessionError> {
            let mut budget = OverviewBudget::standard()?;
            Arc::get_mut(&mut budget.0)
                .ok_or(SessionError::Invalid(
                    "overview test budget is not exclusive",
                ))?
                .before_release = before_release;
            let overview = OriginalSourceOverview::prepare(
                &budget,
                frames,
                rate,
                content,
                &AtomicBool::new(false),
            )?;
            let table = OriginalSourceOverviews::default().inserted(
                &budget,
                asset,
                Arc::clone(&overview),
            )?;
            Ok(Self {
                budget,
                overview: Some(overview),
                table: Some(table),
            })
        }
        pub fn view(&self, context: hex_engine::AudioSourceContext) -> OriginalSourceOverviews {
            self.table
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .at_context(context)
        }
        pub fn usage(&self) -> SourceOverviewUsage {
            self.budget.usage()
        }
        pub fn drop_table(&mut self) {
            drop(self.table.take());
        }
        pub fn drop_overview(&mut self) {
            drop(self.overview.take());
        }
    }
}
