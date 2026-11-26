//! Waveform geometry layout projection and path building.
use super::*;

#[derive(Clone, Copy)]
pub(in crate::editor) struct WaveformLayout {
    pub(super) grid: WaveformGrid,
    pub(super) frames: u64,
    pub(super) first: usize,
    pub(super) last: usize,
    pub(super) point_count: usize,
}

impl WaveformLayout {
    pub(super) fn new(request: &WaveformGeometryRequest<'_>) -> Option<Self> {
        let width = f32::from(request.bounds.size.width);
        let height = f32::from(request.bounds.size.height);
        let grid = request.grid.unwrap_or(WaveformGrid {
            start: request.visible_start,
            end: request.visible_end,
            left: f32::from(request.bounds.origin.x),
            width,
            columns: request.columns,
            binned: false,
        });
        let frames = grid.end.checked_sub(grid.start)?;
        if frames == 0
            || grid.columns == 0
            || grid.columns > MAX_WAVEFORM_COLUMNS
            || !grid.left.is_finite()
            || !grid.width.is_finite()
            || grid.width <= 0.0
            || !height.is_finite()
            || height <= 0.0
        {
            return None;
        }
        // Keep a neighbor on each side for the clip's overflow mask to crop.
        let first = ((u128::from(request.visible_start.saturating_sub(grid.start))
            * grid.columns as u128)
            / u128::from(frames))
        .min(grid.columns as u128) as usize;
        let last = (u128::from(request.visible_end.saturating_sub(grid.start))
            * grid.columns as u128)
            .div_ceil(u128::from(frames))
            .min(grid.columns as u128) as usize;
        let first = first.saturating_sub(1);
        let last = last.saturating_add(1).min(grid.columns);
        let point_count =
            last.saturating_sub(first)
                .checked_mul(if grid.binned { 2 } else { 1 })?;
        if point_count > MAX_WAVEFORM_ENVELOPE_POINTS {
            return None;
        }
        Some(Self {
            grid,
            frames,
            first,
            last,
            point_count,
        })
    }

    pub(super) fn all_columns_exact(self) -> bool {
        self.frames.div_ceil(self.grid.columns as u64) <= self.frame_detail_budget() as u64
    }

    pub(super) fn frame_detail_budget(self) -> usize {
        // Exact sample-by-sample fade/placement belongs to a close view.
        // This matches the ordinary waveform's small exact-read window.
        self.detail_budget().min(64)
    }

    pub(super) fn detail_budget(self) -> usize {
        (4_194_304 / self.grid.columns).min(4_096)
    }

    /// Exact bins survive a viewport's padded grid moving by one or more steps.
    /// The nonintegral fallback keeps its window-specific geometry key.
    pub(super) fn coarse_placement(self, bounds: Bounds<Pixels>) -> Option<CoarsePlacement> {
        let frames_per_bin = self.frames / self.grid.columns as u64;
        if frames_per_bin == 0
            || !self.frames.is_multiple_of(self.grid.columns as u64)
            || self.first >= self.last
        {
            return None;
        }
        let first_frame = self
            .grid
            .start
            .checked_add(frames_per_bin.checked_mul(self.first as u64)?)?;
        let pitch = self.grid.width / self.grid.columns as f32;
        let left = self.grid.left + self.first as f32 * self.grid.width / self.grid.columns as f32;
        let bins = self.last - self.first;
        if pitch <= 0.0
            || !pitch.is_finite()
            || !left.is_finite()
            || !(left + bins as f32 * pitch).is_finite()
            || !f32::from(bounds.origin.y).is_finite()
            || !f32::from(bounds.bottom()).is_finite()
        {
            return None;
        }
        Some(CoarsePlacement {
            key: WaveformPlacementKey::Coarse {
                first_frame,
                frames_per_bin,
                bins,
                pixel_pitch: pitch.to_bits(),
                detail_budget: self.detail_budget(),
            },
            pitch,
            offset: point(px(left), bounds.origin.y),
        })
    }
}

pub(in crate::editor) struct CoarsePlacement {
    pub(super) key: WaveformPlacementKey,
    pub(super) pitch: f32,
    pub(super) offset: Point<Pixels>,
}

#[cfg(test)]
pub(in crate::editor) fn prepare_waveform_geometry(
    waveform: &CachedWaveform,
    mapping: &WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    columns: usize,
    grid: Option<WaveformGrid>,
) -> Option<WaveformGeometry> {
    prepare_waveform_geometry_with_scale(
        waveform,
        &WaveformGeometryRequest {
            mapping,
            amplitude_scale: WaveformAmplitudeScale::default(),
            visible_start,
            visible_end,
            bounds,
            scale: 1.0,
            columns,
            grid,
        },
    )
}

pub(in crate::editor) fn prepare_waveform_geometry_with_scale(
    waveform: &CachedWaveform,
    request: &WaveformGeometryRequest<'_>,
) -> Option<WaveformGeometry> {
    let overrange = if request.mapping.can_prove_overrange() {
        WaveformOverrange::Unavailable
    } else {
        WaveformOverrange::NotApplicable
    };
    build_waveform_geometry(
        waveform,
        request,
        WaveformLayout::new(request)?,
        None,
        overrange,
    )
}

pub(in crate::editor) fn build_waveform_geometry(
    waveform: &CachedWaveform,
    request: &WaveformGeometryRequest<'_>,
    layout: WaveformLayout,
    local_pitch: Option<f32>,
    overrange: WaveformOverrange,
) -> Option<WaveformGeometry> {
    let WaveformGeometryRequest {
        mapping,
        bounds,
        amplitude_scale,
        ..
    } = *request;
    let WaveformLayout {
        grid,
        frames,
        first,
        last,
        point_count,
    } = layout;
    #[cfg(all(test, feature = "ui-test-support"))]
    let _navigation_timer = super::super::navigation_metrics::Timer::start(
        super::super::navigation_metrics::Stage::WaveformBuild,
    );
    let height = f32::from(bounds.size.height);
    let y = if local_pitch.is_some() {
        px(0.0)
    } else {
        bounds.origin.y
    };
    let frame_query;
    let frame = match mapping {
        WaveformMapping::Frame {
            projection, clock, ..
        } => {
            frame_query = projection.query()?;
            Some(clock.resolve(&frame_query)?)
        }
        _ => None,
    };
    let mut range_budget = waveform_frame::RANGE_BUDGET;
    let mut event_fade = match mapping {
        WaveformMapping::MusicalEventFade {
            projection, clock, ..
        } => Some(projection.query(clock)?),
        _ => None,
    };
    let musical = match mapping {
        WaveformMapping::PreparedMusicalFade(fade) => Some(fade.projection()?),
        _ => None,
    };
    let sampled_envelope = (frame.is_some() && mapping.can_prove_overrange())
        || matches!(
            mapping,
            WaveformMapping::PreparedFade(_) | WaveformMapping::PreparedExact(_)
        );
    let mut exact_overrange =
        (sampled_envelope && layout.all_columns_exact()).then(|| Vec::with_capacity(last - first));
    let gain = match mapping {
        WaveformMapping::MusicalEvent(projection) => projection.gain().linear(),
        WaveformMapping::Stretch(projection) => projection.gain().linear(),
        WaveformMapping::Loop(projection) => projection.gain().linear(),
        WaveformMapping::Clip { clip, gain, .. } => gain.unwrap_or_else(|| clip.gain().linear()),
        WaveformMapping::GainedSource { gain, .. } => *gain,
        WaveformMapping::Prepared { gain, .. } => gain.linear(),
        WaveformMapping::Source { .. }
        | WaveformMapping::Frame { .. }
        | WaveformMapping::MusicalEventFade { .. }
        | WaveformMapping::PreparedFade(_)
        | WaveformMapping::PreparedExact(_)
        | WaveformMapping::PreparedMusicalFade(_) => 1.0,
    };
    let clip_fade = match mapping {
        WaveformMapping::Clip {
            clip,
            fade: Some(fade),
            ..
        } if !fade.is_neutral() => Some((
            clip.start(),
            clip.start().saturating_add(clip.duration()),
            *fade,
        )),
        _ => None,
    };
    #[cfg(all(test, feature = "ui-test-support"))]
    let sampling_timer = super::super::navigation_metrics::Timer::start(
        super::super::navigation_metrics::Stage::WaveformSampling,
    );
    let mut stereo_peaks = Vec::with_capacity(last.saturating_sub(first));
    let mut stereo_rms = Vec::with_capacity(last.saturating_sub(first));
    let mut has_any_rms = false;
    // A fixed total bound keeps close-zoom refinement proportional to the
    // viewport, including columns that straddle a loop boundary.
    let detail_budget = layout.detail_budget();
    for column in first..last {
        let start = grid
            .start
            .saturating_add(fraction_u64(frames, column, grid.columns));
        let end = grid
            .start
            .saturating_add(fraction_u64(frames, column + 1, grid.columns))
            .max(start.saturating_add(1));
        let rms_raw = match mapping {
            WaveformMapping::Source { timeline_origin }
            | WaveformMapping::GainedSource {
                timeline_origin, ..
            } => start
                .checked_sub(*timeline_origin)
                .zip(end.checked_sub(*timeline_origin))
                .and_then(|(s, e)| usize::try_from(s).ok().zip(usize::try_from(e).ok()))
                .and_then(|(s, e)| waveform.rms_for_frames(s, e)),
            WaveformMapping::Prepared {
                timeline_origin,
                root_offset,
                ..
            } => {
                let translation = i128::from(*root_offset) - i128::from(*timeline_origin);
                let available = waveform.source_frames() as i128;
                let s = (i128::from(start) + translation).clamp(0, available) as usize;
                let e = (i128::from(end) + translation).clamp(0, available) as usize;
                (s < e).then(|| waveform.rms_for_frames(s, e)).flatten()
            }
            WaveformMapping::Clip {
                clip,
                timeline_origin,
                ..
            } => {
                let ranges = timeline_source_ranges_at(
                    clip,
                    *timeline_origin,
                    start..end,
                    waveform.source_frames(),
                );
                let mut sum_sq = [0.0_f64; 2];
                let mut weight = 0_usize;
                for range in ranges.as_slice() {
                    if let Some(rms) = waveform.rms_for_frames(range.start, range.end) {
                        let w = range.len();
                        sum_sq[0] += f64::from(rms[0]).powi(2) * w as f64;
                        sum_sq[1] += f64::from(rms[1]).powi(2) * w as f64;
                        weight += w;
                    }
                }
                (weight > 0).then(|| {
                    [
                        (sum_sq[0] / weight as f64).sqrt() as f32,
                        (sum_sq[1] / weight as f64).sqrt() as f32,
                    ]
                })
            }
            _ => None,
        };
        let peaks = match mapping {
            WaveformMapping::Frame { projection, .. } => {
                let frame = frame.as_ref()?;
                let frame_detail_budget = layout.frame_detail_budget();
                let peaks =
                    if projection.has_exact_pcm() && end - start <= frame_detail_budget as u64 {
                        waveform_frame::detail(frame, waveform, start..end, frame_detail_budget)?
                    } else {
                        waveform_frame::coarse(
                            frame,
                            waveform,
                            projection.grouped_sources(),
                            start..end,
                            &mut range_budget,
                        )?
                    };
                Some(peaks)
            }
            WaveformMapping::MusicalEventFade { .. } => {
                Some(super::super::waveform_musical_event::peaks(
                    event_fade.as_mut()?,
                    waveform,
                    start..end,
                    &mut range_budget,
                )?)
            }
            WaveformMapping::PreparedFade(fade) => {
                Some(fade.peaks(waveform, start..end, layout.frame_detail_budget())?)
            }
            WaveformMapping::PreparedExact(exact) => {
                Some(exact.peaks(waveform, start..end, layout.frame_detail_budget())?)
            }
            WaveformMapping::PreparedMusicalFade(fade) => {
                Some(fade.peaks(musical.as_ref()?, waveform, start..end)?)
            }
            WaveformMapping::MusicalEvent(projection) => {
                let ranges = projection.ranges(start..end, waveform.source_frames());
                aggregate_ranges(waveform, ranges.as_slice())
            }
            WaveformMapping::Loop(projection) => {
                projection.peaks_with_detail(waveform, start..end, detail_budget)
            }
            WaveformMapping::Stretch(projection) => {
                let ranges = projection.ranges(start..end, waveform.source_frames());
                aggregate_ranges_with_detail(waveform, ranges.as_slice(), detail_budget)
            }
            WaveformMapping::Prepared {
                timeline_origin,
                root_offset,
                ..
            } => {
                // A viewport bin may begin before the cropped clip origin while
                // still addressing the retained full root. Combine the signed
                // translation before clamping; subtracting origin first erases
                // valid edge bins and changes their shape during a left trim.
                let translation = i128::from(*root_offset) - i128::from(*timeline_origin);
                let available = waveform.source_frames() as i128;
                let start = (i128::from(start) + translation).clamp(0, available) as usize;
                let end = (i128::from(end) + translation).clamp(0, available) as usize;
                if waveform.is_repeated() {
                    waveform
                        .bounded_exact_peaks_for_frames(start, end, detail_budget)
                        .or_else(|| aggregate_peaks(waveform, start, end))
                } else {
                    aggregate_ranges_with_detail(
                        waveform,
                        std::slice::from_ref(&(start..end)),
                        detail_budget,
                    )
                }
            }
            WaveformMapping::Clip {
                clip,
                timeline_origin,
                ..
            } => {
                let ranges = timeline_source_ranges_at(
                    clip,
                    *timeline_origin,
                    start..end,
                    waveform.source_frames(),
                );
                aggregate_ranges_with_detail(waveform, ranges.as_slice(), detail_budget)
            }
            WaveformMapping::Source { timeline_origin }
            | WaveformMapping::GainedSource {
                timeline_origin, ..
            } => start
                .checked_sub(*timeline_origin)
                .zip(end.checked_sub(*timeline_origin))
                .and_then(|(start, end)| usize::try_from(start).ok().zip(usize::try_from(end).ok()))
                .and_then(|(start, end)| {
                    // Limit total PCM reads per event geometry, even for a large
                    // viewport. Peak-only sources and wider views use the
                    // existing bounded summary path.
                    const DETAIL_FRAME_BUDGET: usize = 4_194_304;
                    const DETAIL_COLUMN_BUDGET: usize = 4_096;
                    let detail_budget =
                        (DETAIL_FRAME_BUDGET / grid.columns).min(DETAIL_COLUMN_BUDGET);
                    if matches!(mapping, WaveformMapping::GainedSource { detail: true, .. }) {
                        waveform
                            .bounded_exact_peaks_for_frames(start, end, detail_budget)
                            .or_else(|| aggregate_peaks(waveform, start, end))
                    } else {
                        aggregate_peaks(waveform, start, end)
                    }
                }),
        };
        let mut peaks = peaks.unwrap_or([CachedPeak { min: 0.0, max: 0.0 }; 2]);
        let fade_gain = match clip_fade {
            Some((clip_start, clip_end, fade)) => {
                // A cached peak represents an interval; use its timeline
                // midpoint within the clip to sample the envelope without PCM.
                let first = start.max(clip_start);
                let last = end.min(clip_end);
                if first < last {
                    let midpoint = first + (last - first - 1) / 2;
                    fade.gain_at(midpoint - clip_start)
                } else {
                    0.0
                }
            }
            _ => 1.0,
        };
        let gain = gain * fade_gain;
        if gain != 1.0 {
            for peak in &mut peaks {
                peak.min *= gain;
                peak.max *= gain;
            }
        }
        if let Some(columns) = &mut exact_overrange {
            columns.push(Some(waveform_frame::overrange(peaks)));
        }
        let rms = rms_raw.map(|raw| {
            has_any_rms = true;
            let abs_gain = gain.abs();
            [
                (raw[0] * abs_gain).min(peaks[0].max.abs().max(peaks[0].min.abs())),
                (raw[1] * abs_gain).min(peaks[1].max.abs().max(peaks[1].min.abs())),
            ]
        });
        stereo_rms.push(rms);
        stereo_peaks.push(peaks);
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    drop(sampling_timer);
    #[cfg(all(test, feature = "ui-test-support"))]
    let _paths_timer = super::super::navigation_metrics::Timer::start(
        super::super::navigation_metrics::Stage::WaveformPaths,
    );
    let overrange_input = overrange.clone();
    let overrange = exact_overrange.map_or(overrange, |columns| {
        WaveformOverrange::Ready(Arc::new(columns))
    });
    #[cfg(test)]
    let force_stereo = reference_stereo();
    #[cfg(not(test))]
    let force_stereo = false;
    let single_lane = !force_stereo
        && (height < crate::theme::WAVEFORM_STEREO_MIN_HEIGHT || waveform.has_identical_channels());
    let lane_count = if single_lane { 1 } else { 2 };
    let mut channels = Vec::with_capacity(lane_count);
    #[cfg(test)]
    let mut outlines = std::array::from_fn(|_| WaveformChannelGeometry {
        outline: Vec::new(),
    });
    let (guide_left, guide_right) = if let Some(pitch) = local_pitch {
        (px(0.0), px((last - first) as f32 * pitch))
    } else {
        (bounds.origin.x, bounds.origin.x + bounds.size.width)
    };
    for lane in 0..lane_count {
        let band_height = if single_lane { height } else { height * 0.5 };
        let band_top = band_height * lane as f32;
        let center = band_top + band_height * 0.5;
        let amplitude = (band_height * 0.44).max(0.5);
        let mut maxima = Vec::with_capacity(point_count);
        let mut minima = Vec::with_capacity(point_count);
        let mut rms_upper = Vec::with_capacity(if has_any_rms { point_count } else { 0 });
        let mut rms_lower = Vec::with_capacity(if has_any_rms { point_count } else { 0 });
        for (column, peaks) in stereo_peaks.iter().enumerate() {
            let peak = if single_lane {
                CachedPeak {
                    min: peaks[0].min.min(peaks[1].min),
                    max: peaks[0].max.max(peaks[1].max),
                }
            } else {
                peaks[lane]
            };
            let (top, bottom) = peak_vertical_bounds(peak, center, amplitude, amplitude_scale);
            let rms_bounds = has_any_rms.then(|| {
                let rms_val = stereo_rms[column].map_or(0.0, |rms| {
                    if single_lane {
                        ((rms[0] * rms[0] + rms[1] * rms[1]) * 0.5).sqrt()
                    } else {
                        rms[lane]
                    }
                });
                peak_vertical_bounds(
                    CachedPeak {
                        min: -rms_val,
                        max: rms_val,
                    },
                    center,
                    amplitude,
                    amplitude_scale,
                )
            });
            if grid.binned {
                for edge in [column, column + 1] {
                    let x = if let Some(pitch) = local_pitch {
                        edge as f32 * pitch
                    } else {
                        grid.left + (first + edge) as f32 * grid.width / grid.columns as f32
                    };
                    maxima.push(point(px(x), y + px(top)));
                    minima.push(point(px(x), y + px(bottom)));
                    if let Some((r_top, r_bottom)) = rms_bounds {
                        rms_upper.push(point(px(x), y + px(r_top)));
                        rms_lower.push(point(px(x), y + px(r_bottom)));
                    }
                }
            } else {
                let x =
                    grid.left + ((first + column) as f32 + 0.5) * grid.width / grid.columns as f32;
                maxima.push(point(px(x), bounds.origin.y + px(top)));
                minima.push(point(px(x), bounds.origin.y + px(bottom)));
                if let Some((r_top, r_bottom)) = rms_bounds {
                    rms_upper.push(point(px(x), bounds.origin.y + px(r_top)));
                    rms_lower.push(point(px(x), bounds.origin.y + px(r_bottom)));
                }
            }
        }

        let has_area = maxima
            .iter()
            .zip(&minima)
            .any(|(maximum, minimum)| (f32::from(maximum.y - minimum.y)).abs() >= 0.5);
        let guide_y = y + px(center);
        let display = WaveformChannel::new(
            WaveformEnvelope {
                upper: &maxima,
                lower: &minima,
                mode: if has_area {
                    WaveformEnvelopeMode::Filled
                } else {
                    WaveformEnvelopeMode::Trace
                },
                trace_width: px(1.0),
                // Painted as a cheap 1-device-pixel quad in hex-widgets while
                // keeping the paired stereo envelope fast path intact.
                centerline: (guide_right > guide_left).then_some(WaveformStroke {
                    points: [point(guide_left, guide_y), point(guide_right, guide_y)],
                    width: px(1.0),
                }),
            },
            std::iter::empty(),
        )?;
        let display = if has_area && !rms_upper.is_empty() {
            display.with_rms_band(&rms_upper, &rms_lower)?
        } else {
            display
        };
        let display = if let WaveformOverrange::Ready(columns) = &overrange {
            let step = local_pitch.unwrap_or(grid.width / grid.columns as f32);
            let left = if local_pitch.is_some() {
                0.0
            } else {
                grid.left + first as f32 * step
            };
            let (top, bottom) = peak_vertical_bounds(
                CachedPeak {
                    min: -1.0,
                    max: 1.0,
                },
                center,
                amplitude,
                amplitude_scale,
            );
            let spans = columns.iter().enumerate().flat_map(|(column, result)| {
                let start = left + column as f32 * step;
                let end = start + step;
                result.iter().flat_map(move |result| {
                    let positive = if single_lane {
                        result.positive[0] || result.positive[1]
                    } else {
                        result.positive[lane]
                    };
                    let negative = if single_lane {
                        result.negative[0] || result.negative[1]
                    } else {
                        result.negative[lane]
                    };
                    [(positive, top), (negative, bottom)]
                        .into_iter()
                        .filter(move |(marked, _)| *marked && end > start)
                        .map(move |(_, ordinate)| {
                            [
                                point(px(start), y + px(ordinate)),
                                point(px(end), y + px(ordinate)),
                            ]
                        })
                })
            });
            display.with_overrange_markers(spans, px(1.0))?
        } else {
            display
        };
        channels.push(display);
        #[cfg(test)]
        {
            outlines[lane] = WaveformChannelGeometry {
                outline: maxima.into_iter().chain(minima).collect(),
            };
        }
    }
    let decision_columns = match &overrange {
        WaveformOverrange::Ready(columns) => columns.len(),
        _ => 0,
    };
    let display = if single_lane {
        hex_widgets::WaveformGeometry::mono(channels.into_iter().next()?)
    } else {
        hex_widgets::WaveformGeometry::stereo(channels.try_into().ok()?)
    };
    Some(WaveformGeometry {
        display,
        #[cfg(test)]
        overrange,
        overrange_input,
        #[cfg(test)]
        channels: outlines,
        columns: point_count.saturating_add(decision_columns),
    })
}
