//! Asynchronous overrange peak tracking for cached waveform geometry.
use super::*;

#[derive(Default)]
pub(in crate::editor) struct OverrangeCache {
    pub(super) worker: Option<super::super::waveform_overrange::Worker>,
    pub(super) requested: bool,
    pub(super) retry: bool,
    pub(super) unavailable: bool,
    pub(super) next_id: u64,
    pub(super) columns: usize,
    pub(super) pending: usize,
    pub(super) entries: VecDeque<OverrangeEntry>,
}

pub(in crate::editor) struct OverrangeEntry {
    pub(super) id: u64,
    pub(super) key: WaveformGeometryKey,
    pub(super) source: Arc<CachedWaveform>,
    pub(super) columns: usize,
    pub(super) state: OverrangeState,
}

pub(in crate::editor) enum OverrangeState {
    Pending,
    Unavailable,
    Ready(Arc<Vec<Option<Overrange>>>),
}

/// Decision snapshot corresponding to this exact source and display projection.
/// `None` columns remain unknown; they never mean silence or a safe signal.
#[derive(Clone)]
pub(in crate::editor) enum WaveformOverrange {
    NotApplicable,
    Pending,
    Unavailable,
    Ready(Arc<Vec<Option<Overrange>>>),
}

impl WaveformOverrange {
    pub(super) fn same_snapshot(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NotApplicable, Self::NotApplicable)
            | (Self::Pending, Self::Pending)
            | (Self::Unavailable, Self::Unavailable) => true,
            (Self::Ready(a), Self::Ready(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl OverrangeCache {
    pub(super) fn schedule(
        &mut self,
        key: &WaveformGeometryKey,
        waveform: &Arc<CachedWaveform>,
        mapping: &WaveformMapping,
        layout: WaveformLayout,
    ) -> WaveformOverrange {
        // Original-frame requests use effective amplitude identity across
        // held, pending and canonical paints. The legacy clip path still
        // checks committed gain only.
        if matches!(
            mapping,
            WaveformMapping::Frame { .. } | WaveformMapping::PreparedFade(_)
        ) && layout.all_columns_exact()
        {
            return WaveformOverrange::NotApplicable;
        }
        let mapping = match mapping {
            WaveformMapping::Clip {
                clip,
                timeline_origin,
                gain: None,
                fade: Some(fade),
            } => waveform_overrange::Mapping::Clip {
                clip: clip.clone(),
                timeline_origin: *timeline_origin,
                fade: Some(*fade),
            },
            WaveformMapping::Frame {
                projection, clock, ..
            } => waveform_overrange::Mapping::Frame(Arc::clone(projection), *clock),
            WaveformMapping::PreparedFade(fade) => waveform_overrange::Mapping::PreparedFade(*fade),
            _ => return WaveformOverrange::NotApplicable,
        };
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.key == *key && Arc::ptr_eq(&entry.source, waveform))
        {
            if let Some(mut entry) = self.entries.remove(index) {
                if let OverrangeState::Ready(columns) = &entry.state
                    && columns.len() != entry.columns
                {
                    entry.state = OverrangeState::Unavailable;
                }
                let snapshot = match &entry.state {
                    OverrangeState::Pending => WaveformOverrange::Pending,
                    OverrangeState::Unavailable => WaveformOverrange::Unavailable,
                    OverrangeState::Ready(columns) => WaveformOverrange::Ready(Arc::clone(columns)),
                };
                self.entries.push_back(entry);
                return snapshot;
            }
            return WaveformOverrange::Unavailable;
        }
        if self.unavailable {
            return WaveformOverrange::Unavailable;
        }
        if self.pending >= MAX_PENDING_OVERRANGE {
            return WaveformOverrange::Pending;
        }
        let Some(worker) = self.worker.as_mut() else {
            self.requested = true;
            return WaveformOverrange::Pending;
        };
        let count = layout.last.saturating_sub(layout.first);
        if count == 0 || count > MAX_CACHED_WAVEFORM_COLUMNS {
            return WaveformOverrange::Unavailable;
        }
        let Some(id) = self.next_id.checked_add(1) else {
            self.unavailable = true;
            return WaveformOverrange::Unavailable;
        };
        let mut columns = Vec::new();
        if columns.try_reserve_exact(count).is_err() {
            return WaveformOverrange::Unavailable;
        }
        for column in layout.first..layout.last {
            let start = layout.grid.start.saturating_add(fraction_u64(
                layout.frames,
                column,
                layout.grid.columns,
            ));
            let end = layout
                .grid
                .start
                .saturating_add(fraction_u64(layout.frames, column + 1, layout.grid.columns))
                .max(start.saturating_add(1));
            if start >= end {
                return WaveformOverrange::Unavailable;
            }
            columns.push(start..end);
        }
        let request = super::super::waveform_overrange::Request {
            id,
            slot: id,
            waveform: Arc::clone(waveform),
            mapping,
            columns,
        };
        match worker.submit(request) {
            super::super::waveform_overrange::Admission::Queued => {}
            super::super::waveform_overrange::Admission::Full => {
                // A recently cancelled request may still occupy the bounded
                // worker queue after the cache was cleared. Retry on the next
                // repaint even if no current job remains to wake the editor.
                self.retry = true;
                return WaveformOverrange::Pending;
            }
            super::super::waveform_overrange::Admission::Unavailable => {
                return WaveformOverrange::Unavailable;
            }
        }
        self.next_id = id;
        while self.entries.len() >= MAX_CACHED_WAVEFORM_ENTRIES
            || self.columns.saturating_add(count) > MAX_CACHED_WAVEFORM_COLUMNS
        {
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            self.columns -= old.columns;
            if matches!(old.state, OverrangeState::Pending) {
                worker.cancel(old.id);
                self.pending -= 1;
            }
        }
        self.columns += count;
        self.pending += 1;
        self.entries.push_back(OverrangeEntry {
            id,
            key: key.clone(),
            source: Arc::clone(waveform),
            columns: count,
            state: OverrangeState::Pending,
        });
        WaveformOverrange::Pending
    }

    pub(super) fn poll(&mut self) -> bool {
        if self.requested && self.worker.is_none() && !self.unavailable {
            self.requested = false;
            match super::super::waveform_overrange::Worker::new() {
                Ok(worker) => self.worker = Some(worker),
                Err(_) => self.unavailable = true,
            }
            return true;
        }
        let Some(worker) = self.worker.as_mut() else {
            return false;
        };
        // Consume this wake once. A still-full, still-visible request sets it
        // again during the resulting paint; an offscreen request cannot spin.
        let mut changed = std::mem::take(&mut self.retry);
        loop {
            match worker.try_recv() {
                Ok(reply) => {
                    if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == reply.id)
                    {
                        self.pending -= 1;
                        entry.state = match reply.columns {
                            Some(columns) if columns.len() == entry.columns => {
                                OverrangeState::Ready(Arc::new(columns))
                            }
                            _ => OverrangeState::Unavailable,
                        };
                        changed = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.unavailable = true;
                    self.entries.clear();
                    self.columns = 0;
                    self.pending = 0;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    pub(super) fn busy(&self) -> bool {
        self.requested || self.retry || self.pending > 0
    }

    pub(super) fn clear(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            for entry in &self.entries {
                if matches!(entry.state, OverrangeState::Pending) {
                    worker.cancel(entry.id);
                }
            }
        }
        self.entries.clear();
        self.columns = 0;
        self.pending = 0;
        self.requested = false;
        self.retry = false;
    }
}
