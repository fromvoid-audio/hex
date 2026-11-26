//! Waveform geometry cache lookup, invalidation, and key construction.
use super::*;

impl WaveformGeometryCache {
    pub(in crate::editor) fn poll_overrange(&mut self) -> bool {
        self.overrange.poll()
    }

    pub(in crate::editor) fn overrange_busy(&self) -> bool {
        self.overrange.busy()
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn overrange_ready_count(&self) -> usize {
        self.overrange
            .entries
            .iter()
            .filter(|entry| matches!(entry.state, OverrangeState::Ready(_)))
            .count()
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn overrange_exit_observer(
        &self,
    ) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        self.overrange
            .worker
            .as_ref()
            .map(|worker| worker.exit_observer())
    }

    pub(super) fn get(
        &mut self,
        key: &WaveformGeometryKey,
        source: &Arc<CachedWaveform>,
    ) -> Option<Arc<WaveformGeometry>> {
        // The coarse index can reject absent signatures immediately; resident
        // hits use the O(1) hash-indexed LRU and verify exact key/source identity.
        if coarse_entry_key(key, source)
            .is_some_and(|coarse_key| !self.coarse_entry_counts.contains_key(&coarse_key))
        {
            return None;
        }
        let slot = self.entries.find_slot(key, source)?;
        self.entries.move_to_back(slot);
        self.entries.geometry_at(slot)
    }

    pub(super) fn invalidate(&mut self, key: &WaveformGeometryKey, source: &Arc<CachedWaveform>) {
        if let Some(slot) = self.entries.find_slot(key, source)
            && let Some(entry) = self.entries.remove_slot(slot)
        {
            self.columns = self.columns.saturating_sub(entry.columns);
            self.remove_coarse_entry(coarse_entry_key(&entry.key, &entry.source));
        }
    }

    pub(super) fn insert(
        &mut self,
        key: WaveformGeometryKey,
        source: Arc<CachedWaveform>,
        geometry: Arc<WaveformGeometry>,
        columns: usize,
    ) {
        if columns == 0 || columns > MAX_CACHED_WAVEFORM_COLUMNS {
            return;
        }
        // Only one transient stretch snapshot is retained. Ordinary entries keep
        // their existing budget; pointer motion cannot retain a history of
        // copied marker/event metadata for every previous preview position.
        if matches!(key.mapping, WaveformMappingKey::Stretch(_))
            && let Some(removed) = self.entries.remove_stretch()
        {
            self.columns = self.columns.saturating_sub(removed.columns);
            self.remove_coarse_entry(coarse_entry_key(&removed.key, &removed.source));
        }
        while self.entries.len() >= MAX_CACHED_WAVEFORM_ENTRIES
            || self.columns.saturating_add(columns) > MAX_CACHED_WAVEFORM_COLUMNS
        {
            let Some(removed) = self.entries.pop_front() else {
                break;
            };
            self.columns = self.columns.saturating_sub(removed.columns);
            self.remove_coarse_entry(coarse_entry_key(&removed.key, &removed.source));
        }
        self.columns = self.columns.saturating_add(columns);
        let coarse_entry_key = coarse_entry_key(&key, &source);
        if let Some(coarse_key) = coarse_entry_key {
            *self.coarse_entry_counts.entry(coarse_key).or_default() += 1;
        }
        self.entries.push_back(WaveformGeometryEntry {
            key,
            source,
            geometry,
            columns,
        });
    }

    pub(in crate::editor) fn clear(&mut self) {
        self.overrange.clear();
        self.entries.clear();
        self.columns = 0;
        self.coarse_entry_counts.clear();
        #[cfg(all(test, feature = "ui-test-support"))]
        self.painted.clear();
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn record_paint(
        &mut self,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        geometry: &WaveformGeometry,
        offset: Point<Pixels>,
        colors: hex_widgets::WaveformColors,
    ) {
        let _navigation_timer = super::super::navigation_metrics::Timer::start(
            super::super::navigation_metrics::Stage::PaintObservation,
        );
        if self.painted.len() == MAX_CACHED_WAVEFORM_ENTRIES {
            self.painted.pop_front();
        }
        let mut observations: [(f32, f32, u64); 2] = std::array::from_fn(|channel| {
            geometry.channels[channel].outline.iter().fold(
                (f32::INFINITY, f32::NEG_INFINITY, 0xcbf29ce484222325_u64),
                |(lo, hi, hash), point| {
                    let y = f32::from(point.y + offset.y);
                    (
                        lo.min(y),
                        hi.max(y),
                        (hash ^ u64::from(y.to_bits())).wrapping_mul(0x100000001b3),
                    )
                },
            )
        });
        if geometry.channels[1].outline.is_empty() && !geometry.channels[0].outline.is_empty() {
            observations[1] = observations[0];
        }
        self.painted.push_back(WaveformPaintSample {
            colors,
            bounds,
            mask,
            ordinates: observations.map(|(lo, hi, _)| (lo, hi)),
            envelope_points: geometry
                .channels
                .iter()
                .map(|channel| channel.outline.len())
                .sum(),
            outline_fingerprint: observations.map(|(_, _, fingerprint)| fingerprint),
            overrange_ready: matches!(&geometry.overrange, WaveformOverrange::Ready(_)),
        });
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn painted_waveforms(&self) -> &VecDeque<WaveformPaintSample> {
        &self.painted
    }

    /// Reset diagnostic observations without invalidating retained geometry.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn clear_paint_samples(&mut self) {
        self.painted.clear();
    }

    /// Actual immutable source, mapping and path-builder ordinates painted by the cache.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn rendered_waveforms(&self) -> Vec<RenderedWaveformSample> {
        self.entries
            .iter()
            .map(|entry| RenderedWaveformSample {
                source: Arc::clone(&entry.source),
                prepared: match &entry.key.mapping {
                    WaveformMappingKey::Prepared {
                        timeline_origin,
                        root_offset,
                        ..
                    } => Some((*timeline_origin, *root_offset)),
                    WaveformMappingKey::PreparedFade(fade) => {
                        Some((fade.timeline_origin, fade.root_offset))
                    }
                    WaveformMappingKey::Frame(key, waveform_frame::Clock::Arrangement) => {
                        key.prepared_origin()
                    }
                    WaveformMappingKey::MusicalEventFade(projection, None) => {
                        projection.prepared_origin()
                    }
                    _ => None,
                },
                spans: std::array::from_fn(|channel| {
                    let (low, high) = entry.geometry.channels[channel]
                        .outline
                        .iter()
                        .map(|point| f32::from(point.y))
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), y| {
                            (low.min(y), high.max(y))
                        });
                    high - low
                }),
            })
            .collect()
    }

    #[cfg(test)]
    pub(in crate::editor) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn remove_coarse_entry(&mut self, key: Option<CoarseEntryKey>) {
        let Some(key) = key else {
            return;
        };
        match self.coarse_entry_counts.get_mut(&key) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                self.coarse_entry_counts.remove(&key);
            }
            None => debug_assert!(false, "coarse geometry index count missing"),
        }
    }
}

pub(in crate::editor) fn coarse_entry_key(
    key: &WaveformGeometryKey,
    source: &Arc<CachedWaveform>,
) -> Option<CoarseEntryKey> {
    let (clip_id, timeline_origin) = match &key.mapping {
        WaveformMappingKey::Clip {
            id,
            timeline_origin,
            ..
        } => (*id, *timeline_origin),
        WaveformMappingKey::Frame(key, waveform_frame::Clock::Arrangement) => (None, key.start),
        WaveformMappingKey::PreparedFade(fade) => (None, fade.timeline_origin),
        WaveformMappingKey::PreparedMusicalFade(fade) => (None, fade.start()),
        _ => return None,
    };
    let WaveformPlacementKey::Coarse {
        frames_per_bin,
        pixel_pitch,
        ..
    } = &key.placement
    else {
        return None;
    };
    Some(CoarseEntryKey {
        source_address: Arc::as_ptr(source) as usize,
        clip_id,
        timeline_origin,
        frames_per_bin: *frames_per_bin,
        pixel_pitch: *pixel_pitch,
    })
}

pub(in crate::editor) fn cached_waveform_geometry(
    cache: &std::cell::RefCell<WaveformGeometryCache>,
    waveform: &Arc<CachedWaveform>,
    request: &WaveformGeometryRequest<'_>,
) -> Option<(Arc<WaveformGeometry>, Point<Pixels>)> {
    #[cfg(all(test, feature = "ui-test-support"))]
    let _navigation_timer = super::super::navigation_metrics::Timer::start(
        super::super::navigation_metrics::Stage::WaveformLookup,
    );
    let mut key = waveform_geometry_key(
        request.mapping,
        request.visible_start,
        request.visible_end,
        request.bounds,
        request.scale,
    );
    key.amplitude_scale = request.amplitude_scale;
    let mut layout = None;
    let mut pitch = None;
    let mut offset = Point::default();
    if matches!(
        request.mapping,
        WaveformMapping::Clip { .. }
            | WaveformMapping::Frame { .. }
            | WaveformMapping::PreparedFade(_)
            | WaveformMapping::PreparedExact(_)
            | WaveformMapping::PreparedMusicalFade(_)
    ) && request.grid.is_some_and(|grid| grid.binned)
    {
        let columns = WaveformLayout::new(request)?;
        if let Some(local) = columns.coarse_placement(request.bounds) {
            // Plain clips with the same complete source and timeline projection
            // can share the retained local path across tracks. Placement still
            // owns the exact absolute bins, scale, height and sampling budget.
            if let WaveformMappingKey::Clip {
                id,
                transformed: false,
                ..
            } = &mut key.mapping
            {
                *id = None;
            }
            key.placement = local.key;
            pitch = Some(local.pitch);
            offset = local.offset;
        }
        layout = Some(columns);
    }
    if let WaveformPlacementKey::Window { grid, .. } = &mut key.placement {
        *grid = request.grid.map(WaveformGrid::key);
    }
    let mut overrange = WaveformOverrange::NotApplicable;
    if request.mapping.can_prove_overrange() {
        let current = layout.or_else(|| WaveformLayout::new(request))?;
        if let Ok(mut cache) = cache.try_borrow_mut() {
            overrange = cache
                .overrange
                .schedule(&key, waveform, request.mapping, current);
        } else {
            overrange = WaveformOverrange::Unavailable;
        }
        layout = Some(current);
    }
    if let Some(geometry) = cache
        .try_borrow_mut()
        .ok()
        .and_then(|mut cache| cache.get(&key, waveform))
    {
        if geometry.overrange_input.same_snapshot(&overrange) {
            return Some((geometry, offset));
        }
        if let Ok(mut cache) = cache.try_borrow_mut() {
            cache.invalidate(&key, waveform);
        }
    }
    let layout = layout.or_else(|| WaveformLayout::new(request))?;
    let geometry = Arc::new(build_waveform_geometry(
        waveform, request, layout, pitch, overrange,
    )?);
    if let Ok(mut cache) = cache.try_borrow_mut() {
        cache.insert(
            key,
            Arc::clone(waveform),
            Arc::clone(&geometry),
            geometry.columns,
        );
    }
    Some((geometry, offset))
}

pub(in crate::editor) fn waveform_geometry_key(
    mapping: &WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    scale: f32,
) -> WaveformGeometryKey {
    let mapping = match mapping {
        WaveformMapping::PreparedFade(fade) => WaveformMappingKey::PreparedFade(*fade),
        WaveformMapping::PreparedExact(exact) => WaveformMappingKey::PreparedExact(*exact),
        WaveformMapping::PreparedMusicalFade(fade) => {
            WaveformMappingKey::PreparedMusicalFade(Arc::clone(fade))
        }
        WaveformMapping::Frame {
            projection, clock, ..
        } => WaveformMappingKey::Frame(Arc::clone(&projection.key), *clock),
        WaveformMapping::MusicalEventFade {
            projection, clock, ..
        } => WaveformMappingKey::MusicalEventFade(
            Arc::clone(projection),
            match clock {
                super::super::waveform_musical_event::Clock::Arrangement => None,
                // One event pane samples the held or pending descriptor
                // directly, including its envelope and gain.
                super::super::waveform_musical_event::Clock::Event(event) => Some(*event),
            },
        ),
        WaveformMapping::MusicalEvent(projection) => {
            WaveformMappingKey::MusicalEvent(projection.clone())
        }
        WaveformMapping::Stretch(projection) => WaveformMappingKey::Stretch(Arc::clone(projection)),
        WaveformMapping::Loop(projection) => WaveformMappingKey::Loop(Arc::clone(projection)),
        WaveformMapping::Prepared {
            timeline_origin,
            root_offset,
            gain,
        } => WaveformMappingKey::Prepared {
            timeline_origin: *timeline_origin,
            root_offset: *root_offset,
            gain: gain.millidb(),
        },
        WaveformMapping::GainedSource {
            timeline_origin,
            gain,
            detail,
        } => WaveformMappingKey::GainedSource {
            timeline_origin: *timeline_origin,
            gain: gain.to_bits(),
            detail: *detail,
        },
        WaveformMapping::Clip {
            clip,
            timeline_origin,
            gain,
            fade,
        } => WaveformMappingKey::Clip {
            id: Some(clip.id().value()),
            transformed: clip.audio_edit().is_some(),
            gain: gain.unwrap_or_else(|| clip.gain().linear()).to_bits(),
            start: clip.start(),
            duration: clip.duration(),
            source_offset: clip.source_offset(),
            source_length: clip.source_length(),
            source_phase: clip.source_phase(),
            looping: clip.looping(),
            timeline_origin: *timeline_origin,
            fade: *fade,
        },
        WaveformMapping::Source { timeline_origin } => WaveformMappingKey::Source {
            timeline_origin: *timeline_origin,
        },
    };
    WaveformGeometryKey {
        #[cfg(test)]
        reference_stereo: reference_stereo(),
        mapping,
        amplitude_scale: WaveformAmplitudeScale::default(),
        height: f32::from(bounds.size.height).to_bits(),
        scale: scale.to_bits(),
        placement: WaveformPlacementKey::Window {
            visible_start,
            visible_end,
            origin_x: f32::from(bounds.origin.x).to_bits(),
            origin_y: f32::from(bounds.origin.y).to_bits(),
            width: f32::from(bounds.size.width).to_bits(),
            grid: None,
        },
    }
}
