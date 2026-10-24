//! Pure typed-command construction for musical audio controls.
use hex_project::{
    BeatTime, BeatsPreserve, BeatsTail, ClipId, Command, SourceGainPoint, WarpMarker, WarpMode,
};
#[derive(Clone, Copy)]
pub(super) enum Action {
    Pitch(i16),
    Anchor,
    Warp,
    SelectMode(u8),
    Grain(i16),
    Fluctuation(i8),
    Seed,
    Preserve,
    SelectTail(BeatsTail),
    Decay(i8),
    Formants(i8),
    Resolution(i16),
    MarkerAdd,
    MarkerFrame(usize, i8),
    MarkerBeat(usize, i8),
    MarkerRemove(usize),
    EnvelopeAdd,
    EnvelopeFrame(usize, i8),
    EnvelopeGain(usize, i8),
    EnvelopeRemove(usize),
    Page(i8),
}
pub(super) fn mode_name(mode: WarpMode) -> &'static str {
    match mode {
        WarpMode::RePitch => "re-pitch",
        WarpMode::Beats { .. } => "beats",
        WarpMode::Tones { .. } => "tones",
        WarpMode::Texture { .. } => "texture",
        WarpMode::Complex => "complex",
        WarpMode::ComplexPro { .. } => "complex pro",
    }
}
pub(super) fn modes() -> [WarpMode; 6] {
    [
        WarpMode::Beats {
            preserve: BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
        WarpMode::RePitch,
        WarpMode::Tones { grain_ms: 40 },
        WarpMode::Texture {
            grain_ms: 40,
            fluctuation: 25,
            seed: 1,
        },
        WarpMode::Complex,
        WarpMode::ComplexPro {
            formants: 100,
            envelope: 128,
        },
    ]
}
pub(super) fn command(
    project: &hex_project::Project,
    id: ClipId,
    action: Action,
) -> Result<Command, hex_project::Invalid> {
    use hex_project::Invalid;
    let clip = project
        .clip(id)
        .ok_or(Invalid("audio clip no longer exists"))?;
    let domain = clip.audio_edit().map(|e| e.domain.as_ref());
    match action {
        Action::Pitch(delta) => {
            return Ok(Command::SetAudioClipPitch {
                clip: id,
                cents: domain
                    .map_or(0, |d| d.pitch_cents)
                    .saturating_add(delta)
                    .clamp(-4_800, 4_800),
            });
        }
        Action::Anchor => {
            return Ok(Command::SetAudioClipMusical {
                clip: id,
                enabled: clip
                    .audio_edit()
                    .is_none_or(|e| e.live_musical_start().is_none()),
            });
        }
        Action::Warp => {
            return Ok(Command::SetAudioClipWarp {
                clip: id,
                settings: if domain.is_some_and(|d| d.warp_enabled) {
                    None
                } else {
                    Some(
                        if let Some(settings) = domain.and_then(|d| d.warp.as_ref()) {
                            settings.clone()
                        } else {
                            hex_project::default_audio_warp(clip, project.tempo_map(), modes()[0])?
                        },
                    )
                },
            });
        }
        Action::EnvelopeAdd
        | Action::EnvelopeFrame(..)
        | Action::EnvelopeGain(..)
        | Action::EnvelopeRemove(..) => {
            let mut points = domain.map_or_else(Vec::new, |d| d.source_gain.clone());
            let frames = project
                .asset(clip.asset())
                .ok_or(Invalid("audio asset missing"))?
                .playback_frames();
            match action {
                Action::EnvelopeAdd => {
                    if points.len() >= hex_project::MAX_AUDIO_POINTS {
                        return Err(Invalid("source envelope is limited to 1024 points"));
                    }
                    if points.is_empty() {
                        points = vec![
                            SourceGainPoint {
                                frame: 0,
                                gain_q16: 65_536,
                            },
                            SourceGainPoint {
                                frame: frames,
                                gain_q16: 65_536,
                            },
                        ];
                    } else if points.len() == 1 {
                        let frame = if points[0].frame == 0 { frames } else { 0 };
                        points.push(SourceGainPoint {
                            frame,
                            gain_q16: 65_536,
                        });
                        points.sort_unstable_by_key(|point| point.frame);
                    } else {
                        let pair = points
                            .windows(2)
                            .max_by_key(|p| p[1].frame - p[0].frame)
                            .ok_or(Invalid("add a second envelope endpoint first"))?;
                        let frame = pair[0].frame + (pair[1].frame - pair[0].frame) / 2;
                        if frame == pair[0].frame {
                            return Err(Invalid("no space between source gain points"));
                        }
                        points.push(SourceGainPoint {
                            frame,
                            gain_q16: (pair[0].gain_q16 / 2) + (pair[1].gain_q16 / 2),
                        });
                        points.sort_unstable_by_key(|p| p.frame);
                    }
                }
                Action::EnvelopeFrame(index, direction) => {
                    let point = points
                        .get_mut(index)
                        .ok_or(Invalid("source gain point missing"))?;
                    point.frame = if direction < 0 {
                        point.frame.saturating_sub(1)
                    } else {
                        point.frame.saturating_add(1).min(frames)
                    };
                }
                Action::EnvelopeGain(index, direction) => {
                    let point = points
                        .get_mut(index)
                        .ok_or(Invalid("source gain point missing"))?;
                    point.gain_q16 = if direction < 0 {
                        point.gain_q16.saturating_sub(6_554)
                    } else {
                        point
                            .gain_q16
                            .saturating_add(6_554)
                            .min(hex_project::MAX_SOURCE_GAIN)
                    };
                }
                Action::EnvelopeRemove(index) => {
                    if index >= points.len() {
                        return Err(Invalid("source gain point missing"));
                    }
                    points.remove(index);
                }
                _ => {}
            }
            return Ok(Command::SetAudioClipSourceGain { clip: id, points });
        }
        _ => {}
    }
    let mut settings = domain
        .and_then(|d| d.warp.as_ref())
        .ok_or(Invalid("enable warp to edit its mode or markers"))?
        .clone();
    match action {
        Action::SelectMode(index) => {
            let mode = *modes()
                .get(usize::from(index))
                .ok_or(Invalid("warp algorithm missing"))?;
            if std::mem::discriminant(&mode) != std::mem::discriminant(&settings.mode) {
                settings.mode = mode;
            }
        }
        Action::Grain(delta) => match &mut settings.mode {
            WarpMode::Tones { grain_ms } | WarpMode::Texture { grain_ms, .. } => {
                *grain_ms = (*grain_ms as i32 + i32::from(delta)).clamp(8, 200) as u16
            }
            _ => return Err(Invalid("mode has no grain size")),
        },
        Action::Fluctuation(delta) => {
            if let WarpMode::Texture { fluctuation, .. } = &mut settings.mode {
                *fluctuation = (*fluctuation as i16 + i16::from(delta)).clamp(0, 100) as u8;
            }
        }
        Action::Seed => {
            if let WarpMode::Texture { seed, .. } = &mut settings.mode {
                *seed = seed.wrapping_add(1);
            }
        }
        Action::Preserve => {
            if let WarpMode::Beats { preserve, .. } = &mut settings.mode {
                *preserve = match preserve {
                    BeatsPreserve::Transients => {
                        BeatsPreserve::Division(BeatTime::from_raw(BeatTime::ONE.raw() / 4))
                    }
                    BeatsPreserve::Division(beat) if beat.raw() < BeatTime::ONE.raw() * 2 => {
                        BeatsPreserve::Division(BeatTime::from_raw(beat.raw() * 2))
                    }
                    _ => BeatsPreserve::Transients,
                };
            }
        }
        Action::SelectTail(selected) => {
            if let WarpMode::Beats { tail, .. } = &mut settings.mode {
                *tail = selected;
            } else {
                return Err(Invalid("select beats to edit transient looping"));
            }
        }
        Action::Decay(delta) => {
            if let WarpMode::Beats { decay, .. } = &mut settings.mode {
                *decay = (*decay as i16 + i16::from(delta)).clamp(0, 100) as u8;
            }
        }
        Action::Formants(delta) => {
            if let WarpMode::ComplexPro { formants, .. } = &mut settings.mode {
                *formants = (*formants as i16 + i16::from(delta)).clamp(0, 100) as u8;
            }
        }
        Action::Resolution(delta) => {
            if let WarpMode::ComplexPro { envelope, .. } = &mut settings.mode {
                *envelope = (*envelope as i32 + i32::from(delta)).clamp(32, 512) as u16;
            }
        }
        Action::MarkerAdd => {
            if settings.markers.len() >= hex_project::MAX_AUDIO_POINTS {
                return Err(Invalid("warp is limited to 1024 markers"));
            }
            let pair = settings
                .markers
                .windows(2)
                .max_by_key(|p| p[1].source_frame - p[0].source_frame)
                .ok_or(Invalid("warp endpoints missing"))?;
            let marker = WarpMarker {
                source_frame: pair[0].source_frame
                    + (pair[1].source_frame - pair[0].source_frame) / 2,
                beat: BeatTime::from_raw(
                    pair[0].beat.raw() + (pair[1].beat.raw() - pair[0].beat.raw()) / 2,
                ),
            };
            settings.markers.push(marker);
            settings.markers.sort_unstable_by_key(|m| m.source_frame);
        }
        Action::MarkerFrame(index, direction) => {
            if index == 0 || index + 1 >= settings.markers.len() {
                return Err(Invalid("source endpoints stay fixed"));
            }
            let marker = &mut settings.markers[index];
            marker.source_frame = if direction < 0 {
                marker.source_frame.saturating_sub(1)
            } else {
                marker.source_frame.saturating_add(1)
            };
        }
        Action::MarkerBeat(index, direction) => {
            if index == 0 {
                return Err(Invalid("first warp beat stays zero"));
            }
            let marker = settings
                .markers
                .get_mut(index)
                .ok_or(Invalid("warp marker missing"))?;
            let step = BeatTime::ONE.raw() / 64;
            marker.beat = BeatTime::from_raw(if direction < 0 {
                marker.beat.raw().saturating_sub(step)
            } else {
                marker.beat.raw().saturating_add(step)
            });
        }
        Action::MarkerRemove(index) => {
            if index == 0 || index + 1 >= settings.markers.len() {
                return Err(Invalid("warp endpoints cannot be removed"));
            }
            settings.markers.remove(index);
        }
        _ => return Err(Invalid("unsupported musical audio control")),
    }
    Ok(Command::SetAudioClipWarp {
        clip: id,
        settings: Some(settings),
    })
}

/// Every control declares its group semantics here; source indices are local.
/// No wildcard: adding an action requires choosing its selection behavior.
pub(super) fn selection_command(
    project: &hex_project::Project,
    id: ClipId,
    action: Action,
) -> Result<Command, hex_project::Invalid> {
    use hex_project::{ClipEdit, WarpAdjustment};
    let edit = match action {
        Action::Pitch(cents) => ClipEdit::PitchBy { cents },
        Action::Warp => ClipEdit::WarpEnabled {
            enabled: project
                .clip(id)
                .ok_or(hex_project::Invalid("audio clip no longer exists"))?
                .audio_edit()
                .is_none_or(|edit| !edit.domain.warp_enabled),
        },
        Action::SelectMode(index) => ClipEdit::WarpMode {
            mode: *modes()
                .get(usize::from(index))
                .ok_or(hex_project::Invalid("warp algorithm missing"))?,
        },
        Action::Grain(delta) => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Grain(delta),
        },
        Action::Fluctuation(delta) => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Fluctuation(delta),
        },
        Action::Seed => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Seed,
        },
        Action::Preserve => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Preserve,
        },
        Action::SelectTail(tail) => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Tail(tail),
        },
        Action::Decay(delta) => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Decay(delta),
        },
        Action::Formants(delta) => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Formants(delta),
        },
        Action::Resolution(delta) => ClipEdit::WarpAdjust {
            adjustment: WarpAdjustment::Resolution(delta),
        },
        Action::Anchor
        | Action::MarkerAdd
        | Action::MarkerFrame(..)
        | Action::MarkerBeat(..)
        | Action::MarkerRemove(_)
        | Action::EnvelopeAdd
        | Action::EnvelopeFrame(..)
        | Action::EnvelopeGain(..)
        | Action::EnvelopeRemove(_)
        | Action::Page(_) => {
            // Scalar commands are widened by the central property resolver.
            // Indexed source commands retain their explicit source scope.
            return command(project, id, action);
        }
    };
    Ok(Command::EditClips {
        clips: vec![id],
        edit,
    })
}
