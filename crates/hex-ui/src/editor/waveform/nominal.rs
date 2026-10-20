//! Current-capability event waveform display; only scalar display facts survive a read.
use super::*;
use crate::editor::EditorView;
use crate::editor::waveform_geometry::{
    WaveformColumns, WaveformVersion, geometry_from_peaks, retain_versioned_waveform_geometry,
    versioned_waveform_geometry, waveform_columns,
};
use hex_session::snapshot_exports::{SnapshotRead, SnapshotSourceKey, SnapshotTransportError};

#[derive(Clone, Copy)]
pub(in crate::editor) struct EventSegmentRequest {
    pub clip: hex_project::ClipId,
    pub source_root_start: u64,
    pub source_root_end: u64,
    pub source_project_start: u64,
    pub source_project_end: u64,
    pub generation: u64,
    pub revision: u64,
    pub source_key: SnapshotSourceKey,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mapping {
    Prepared { gain: hex_project::ClipGain },
    Asset(ClipMapping),
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct SourceFacts {
    version: WaveformVersion,
    mapping: Mapping,
    prepared: Option<(u64, u64)>,
}
impl Mapping {
    fn display(self) -> WaveformMapping {
        match self {
            Self::Prepared { gain } => WaveformMapping::GainedSource {
                timeline_origin: 0,
                gain,
            },
            Self::Asset(clip) => WaveformMapping::Clip {
                timeline_origin: clip.start(),
                clip,
            },
        }
    }
    fn gain(self) -> f32 {
        match self {
            Self::Prepared { gain } => gain.linear(),
            Self::Asset(clip) => clip.gain().linear(),
        }
    }
}
fn source<'a>(
    snapshot: SnapshotRead<'a>,
    request: EventSegmentRequest,
) -> Option<(SourceFacts, hex_session::WaveformRead<'a>)> {
    if snapshot.revision() != request.revision {
        return None;
    }
    let clip = snapshot.model().clip(request.clip)?;
    let (waveform, mapping, prepared) = if let Some(prepared) = snapshot.clip_waveform(request.clip)
    {
        (
            prepared.waveform(),
            Mapping::Prepared { gain: clip.gain() },
            Some((prepared.offset(), prepared.length())),
        )
    } else {
        // Edited roots require their accepted prepared summary. Raw source peaks
        // cannot stand in for an absent processed root.
        if clip.audio_edit().is_some() {
            return None;
        }
        (
            snapshot.waveform(clip.asset())?,
            Mapping::Asset(ClipMapping::from(clip)),
            None,
        )
    };
    Some((
        SourceFacts {
            version: WaveformVersion {
                generation: request.generation,
                identity: waveform.cache_identity()?,
                frames: waveform.source_frames(),
                prepared,
            },
            mapping,
            prepared,
        },
        waveform,
    ))
}

pub(in crate::editor) fn event_segment_view(
    view: &EditorView,
    cx: &Context<EditorView>,
    request: EventSegmentRequest,
) -> Result<Option<Div>, SnapshotTransportError> {
    if request.generation != view.project_generation {
        return Ok(None);
    }
    let Some(state) = view.state.as_ref() else {
        return Ok(None);
    };
    if state.source_key() != Some(request.source_key) {
        return Ok(None);
    }
    let Some(expected) =
        state.read_with(|snapshot| source(snapshot, request).map(|(facts, _)| facts))?
    else {
        return Ok(None);
    };
    let entity = cx.weak_entity();
    Ok(Some(super::waveform_surface(canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (visible_start, visible_end) = match expected.mapping {
                Mapping::Prepared { .. } => (request.source_root_start, request.source_root_end),
                Mapping::Asset(_) => (request.source_project_start, request.source_project_end),
            };
            let columns =
                waveform_column_count(f32::from(bounds.size.width), window.scale_factor());
            let Some(layout) = waveform_columns(visible_start, visible_end, bounds, columns, None)
            else {
                return;
            };
            let mapping = expected.mapping.display();
            let geometry_request = WaveformGeometryRequest {
                mapping: &mapping,
                visible_start,
                visible_end,
                bounds,
                scale: window.scale_factor(),
                columns,
                grid: None,
            };
            let prepared = entity
                .read_with(cx, |view, _| {
                    if view.project_generation != request.generation {
                        return None;
                    }
                    let state = view.state.as_ref()?;
                    if state.source_key() != Some(request.source_key) {
                        return None;
                    }
                    state
                        .read_with(|snapshot| {
                            let (facts, _) = source(snapshot, request)?;
                            (facts == expected).then_some(())
                        })
                        .ok()
                        .flatten()?;
                    Some(std::rc::Rc::clone(&view.waveform_geometry_cache))
                })
                .ok()
                .flatten();
            let Some(cache) = prepared else {
                return;
            };
            let geometry = versioned_waveform_geometry(&cache, expected.version, &geometry_request)
                .or_else(|| {
                    // On a miss reserve bounded Copy scratch outside the family
                    // guard; a hit performs no peak traversal or scratch allocation.
                    let mut peaks = Vec::new();
                    peaks
                        .try_reserve_exact(layout.last.saturating_sub(layout.first))
                        .ok()?;
                    entity
                        .read_with(cx, |view, _| {
                            if view.project_generation != request.generation {
                                return None;
                            }
                            let state = view.state.as_ref()?;
                            if state.source_key() != Some(request.source_key) {
                                return None;
                            }
                            state
                                .read_with(|snapshot| {
                                    let (facts, waveform) = source(snapshot, request)?;
                                    (facts == expected
                                        && fill_peaks(waveform, facts.mapping, layout, &mut peaks))
                                    .then_some(())
                                })
                                .ok()
                                .flatten()
                        })
                        .ok()
                        .flatten()?;
                    let geometry = Arc::new(geometry_from_peaks(
                        bounds,
                        layout.grid,
                        layout.first,
                        &peaks,
                    )?);
                    retain_versioned_waveform_geometry(
                        &cache,
                        expected.version,
                        &geometry_request,
                        Arc::clone(&geometry),
                    );
                    Some(geometry)
                });
            let Some(geometry) = geometry else {
                return;
            };
            #[cfg(all(test, feature = "ui-test-support"))]
            if let Ok(mut cache) = cache.try_borrow_mut() {
                cache.record_paint(bounds, window.content_mask().bounds, &geometry);
            }
            for channel in &geometry.channels {
                window.paint_path(
                    channel.centerline.clone(),
                    rgb(theme::color(theme::PaletteRole::Border)),
                );
                window.paint_path(channel.waveform.clone(), rgb(0x000000));
            }
        },
    ))))
}
fn fill_peaks(
    waveform: hex_session::WaveformRead<'_>,
    mapping: Mapping,
    layout: WaveformColumns,
    peaks: &mut Vec<[CachedPeak; 2]>,
) -> bool {
    let Some(frames) = layout.grid.end.checked_sub(layout.grid.start) else {
        return false;
    };
    let gain = mapping.gain();
    for column in layout.first..layout.last {
        let start =
            layout
                .grid
                .start
                .saturating_add(fraction_u64(frames, column, layout.grid.columns));
        let end = layout
            .grid
            .start
            .saturating_add(fraction_u64(frames, column + 1, layout.grid.columns))
            .max(start.saturating_add(1));
        let value = match mapping {
            Mapping::Prepared { .. } => usize::try_from(start)
                .ok()
                .zip(usize::try_from(end).ok())
                .and_then(|(start, end)| waveform.peaks_for_frames(start, end)),
            Mapping::Asset(clip) => {
                let ranges = timeline_source_ranges_at(
                    &clip,
                    clip.start(),
                    start..end,
                    waveform.source_frames(),
                );
                let mut merged: Option<[CachedPeak; 2]> = None;
                for range in ranges.as_slice() {
                    let Some(value) = waveform.peaks_for_frames(range.start, range.end) else {
                        return false;
                    };
                    if let Some(prior) = merged.as_mut() {
                        for channel in 0..2 {
                            prior[channel].min = prior[channel].min.min(value[channel].min);
                            prior[channel].max = prior[channel].max.max(value[channel].max);
                        }
                    } else {
                        merged = Some(value);
                    }
                }
                merged
            }
        };
        let mut value = value.unwrap_or([CachedPeak { min: 0.0, max: 0.0 }; 2]);
        if gain != 1.0 {
            for peak in &mut value {
                peak.min *= gain;
                peak.max *= gain;
            }
        }
        if peaks.len() == peaks.capacity() {
            return false;
        }
        peaks.push(value);
    }
    true
}
