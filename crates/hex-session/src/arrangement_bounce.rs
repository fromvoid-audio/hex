//! Control-owned selected Arrangement bounce. Rendering and durable create-new
//! media finish before one reserved project/media/history adoption. Originals
//! remain untouched; failed/cancelled jobs remove only their own unpublished files.
use super::*;
use hex_project::{BounceFile, BounceTarget, BounceTrack, TrackContentSnapshot};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
mod console;
pub(crate) mod streamed;
pub use streamed::{StreamedBounceRequest, StreamedBouncer};
static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

pub(crate) fn isolated(
    project: &Project,
    target: &BounceTarget,
    item: &BounceTrack,
    in_place: bool,
) -> Result<Project, String> {
    let mut snapshot = project.snapshot();
    let selected_track_id = item.track.value();
    let selected_instrument_id = snapshot
        .tracks
        .iter()
        .find(|track| track.id == selected_track_id)
        .and_then(|track| track.instrument.as_ref().map(|instrument| instrument.id));
    let selected_source = snapshot.native_sources.iter().find(|source| {
        selected_instrument_id.is_some_and(|instrument_id| {
            source
                .devices
                .get(source.instrument_index)
                .is_some_and(|device| device.id == instrument_id)
        })
    });
    let selected_source_instrument_id = selected_source
        .and_then(|source| source.devices.get(source.instrument_index))
        .map(|device| device.id);
    let selected_is_receiver =
        selected_source.is_some_and(|source| source.receiver == Some(selected_track_id));

    if target.is_audio_group() {
        let selected: std::collections::BTreeSet<_> =
            item.clips.iter().map(|id| id.value()).collect();
        // Keep the group hierarchy valid, but remove every non-source signal.
        // Grouping bakes clip processing only; track/source devices, mixer
        // automation, sends, routing and activation remain live on adoption.
        for track in &mut snapshot.tracks {
            let audio = if track.id == item.track.value() {
                track
                    .audio_clips_mut()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .filter(|c| selected.contains(&c.id))
                    .cloned()
                    .map(|mut c| {
                        c.active = true;
                        c
                    })
                    .collect()
            } else {
                Vec::new()
            };
            track.content = TrackContentSnapshot::Hybrid {
                audio_clips: audio,
                midi_clips: Vec::new(),
            };
            track.devices.clear();
            track.instrument = None;
            track.instrument_position = None;
            track.gain = 1.0;
            track.balance = 0.0;
            track.muted = false;
            track.soloed = false;
            track.gain_envelope.clear();
            track.gain_touch_spans.clear();
            track.sends.clear();
        }
    } else {
        let console_source = matches!(item.source, hex_project::BounceSource::Console { .. });
        if console_source {
            let track = snapshot
                .tracks
                .iter_mut()
                .find(|track| track.id == selected_track_id)
                .ok_or("bounce track changed")?;
            track.content = console::content(project, item, &mut snapshot.next_id)?;
        }
        let selected_midi = snapshot
            .tracks
            .iter()
            .find(|track| track.id == selected_track_id)
            .map(|track| match &track.content {
                TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips
                    .iter()
                    .map(|clip| clip.id)
                    .collect::<std::collections::BTreeSet<_>>(),
                _ => std::collections::BTreeSet::new(),
            })
            .ok_or("bounce track changed")?;
        let has_selected_midi = item
            .clips
            .iter()
            .any(|clip| selected_midi.contains(&clip.value()));
        // For a range, earlier notes may still be sounding at its start. Keep
        // the selected track's MIDI for causal preroll, excluding its peers.
        let render_contributor = !selected_is_receiver
            && selected_source_instrument_id.is_some()
            && match target {
                BounceTarget::Clips(_) => !in_place && has_selected_midi,
                BounceTarget::Range(_) => !in_place && !selected_midi.is_empty(),
                BounceTarget::Track(_) => in_place && !selected_midi.is_empty(),
                _ => false,
            };
        if !selected_is_receiver
            && !render_contributor
            && (item
                .clips
                .iter()
                .any(|clip| selected_midi.contains(&clip.value()))
                // A range may include a shared voice's release from an earlier
                // onset. There is no independent contributor audio stem.
                || matches!(target, BounceTarget::Range(_)) && !selected_midi.is_empty())
        {
            return Err(
                "cannot bounce midi from a shared-source contributor; bounce its receiver instead"
                    .into(),
            );
        }

        // Clip and range bounces print only the selected track's input to
        // the shared runtime, not the other tracks' mixed signal.
        let retain_contributors = !console_source
            && selected_is_receiver
            && selected_source_instrument_id.is_some()
            // A whole-track receiver bounce prints only its own input; linked
            // tracks continue through the retained live source after adoption.
            && !matches!(target, BounceTarget::Track(_))
            && (in_place || !matches!(target, BounceTarget::Clips(_) | BounceTarget::Range(_)));
        snapshot.tracks.retain(|track| {
            track.id == selected_track_id
                || (retain_contributors
                    && track.instrument.as_ref().is_some_and(|instrument| {
                        Some(instrument.id) == selected_source_instrument_id
                    }))
        });
        let track_index = snapshot
            .tracks
            .iter()
            .position(|track| track.id == selected_track_id)
            .ok_or("bounce track changed")?;
        if matches!(target, BounceTarget::Clips(_)) {
            let selected = item
                .clips
                .iter()
                .map(|id| id.value())
                .collect::<std::collections::BTreeSet<_>>();
            for track in &mut snapshot.tracks {
                if track.id == selected_track_id {
                    match &mut track.content {
                        TrackContentSnapshot::Hybrid {
                            audio_clips,
                            midi_clips,
                        } => {
                            audio_clips.retain(|c| selected.contains(&c.id));
                            midi_clips.retain(|c| selected.contains(&c.id));
                        }
                        _ => return Err("bounce requires a hybrid track".into()),
                    }
                } else {
                    track.audio_clips_mut().map_err(|e| e.to_string())?.clear();
                }
            }
        }
        if render_contributor {
            // The copied graph gets its own receiver; the canonical source and
            // live graph retain their original receiver and state.
            let source = snapshot
                .native_sources
                .iter_mut()
                .find(|source| {
                    source
                        .devices
                        .get(source.instrument_index)
                        .is_some_and(|device| Some(device.id) == selected_source_instrument_id)
                })
                .ok_or("bounce source changed")?;
            source.receiver = Some(selected_track_id);
        }
        for (index, track) in snapshot.tracks.iter_mut().enumerate() {
            if index == track_index {
                track.gain = 1.0;
                track.balance = 0.0;
                track.muted = false;
                track.soloed = false;
                track.gain_envelope.clear();
                track.gain_touch_spans.clear();
                track.sends.clear();
                if !selected_is_receiver && !render_contributor {
                    let audio_clips = track.audio_clips_mut().map_err(|e| e.to_string())?.clone();
                    track.instrument = None;
                    track.instrument_position = None;
                    track.content = TrackContentSnapshot::Hybrid {
                        audio_clips,
                        midi_clips: Vec::new(),
                    };
                }
            } else if retain_contributors {
                // Shared MIDI contributors feed the selected receiver's one
                // source instance. Their local audio and effects are not part
                // of that path.
                let midi_clips = match &mut track.content {
                    TrackContentSnapshot::Hybrid { midi_clips, .. } => std::mem::take(midi_clips),
                    _ => return Err("bounce requires a hybrid contributor track".into()),
                };
                track.devices.clear();
                track.instrument_position = Some(0);
                track.content = TrackContentSnapshot::Hybrid {
                    audio_clips: Vec::new(),
                    midi_clips,
                };
                track.gain = 1.0;
                track.balance = 0.0;
                track.muted = false;
                track.soloed = false;
                track.gain_envelope.clear();
                track.gain_touch_spans.clear();
                track.sends.clear();
            }
        }
    }
    for source in &mut snapshot.native_sources {
        let Some(receiver) = source.receiver else {
            continue;
        };
        let instrument_id = source
            .devices
            .get(source.instrument_index)
            .map(|device| device.id);
        let retained_receiver = snapshot.tracks.iter().any(|track| {
            track.id == receiver
                && track
                    .instrument
                    .as_ref()
                    .is_some_and(|instrument| Some(instrument.id) == instrument_id)
        });
        if !retained_receiver {
            source.receiver = None;
        }
    }
    snapshot.session = Default::default();
    snapshot.return_tracks.clear();
    snapshot.main_devices.clear();
    snapshot.audio_feeds.clear();
    snapshot.midi_routes.clear();
    snapshot.return_feeds.clear();
    snapshot.loop_region = None;
    snapshot.master_gain = 1.0;
    snapshot.retain_clip_aliases_after_projection();
    snapshot.retain_automatic_track_names_after_projection();
    // Snapshots exclude runtime availability; offline clips stay silent here too.
    let isolated = Project::from_snapshot(snapshot).map_err(|e| e.to_string())?;
    let offline: Vec<_> = project
        .offline_assets()
        .filter(|id| isolated.asset(*id).is_some())
        .collect();
    isolated
        .with_offline_assets(offline)
        .map_err(|e| e.to_string())
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bounce_arrangement_using<R, T>(
        &mut self,
        target: BounceTarget,
        in_place: bool,
        directory: &Path,
        control: &hex_export::ExportControl,
        mut prepare: impl FnMut(&Session, &Project, u64) -> Result<R, String>,
        mut render: impl FnMut(
            &Path,
            R,
            &BounceTrack,
            bool,
            &hex_export::ExportControl,
        ) -> Result<u64, String>,
        commit: impl FnOnce(&mut Session, Vec<PreparedImport>, Vec<BounceFile>) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.audio_mode != DocumentAudioMode::Resident {
            return Err("arrangement bounce requires resident audio".into());
        }
        let plan = self
            .project()
            .plan_arrangement_bounce(&target, in_place)
            .map_err(|e| e.to_string())?;
        if self
            .media
            .len()
            .checked_add(plan.len())
            .is_none_or(|n| n > MAX_ASSETS)
        {
            return Err("bounce exceeds resident asset limit".into());
        }
        fs::create_dir_all(directory)
            .map_err(|e| format!("could not create bounce folder: {e}"))?;
        let directory = fs::canonicalize(directory).map_err(|e| e.to_string())?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let folder = directory.join(format!(
            "arrangement-{}-{nonce}-{}",
            std::process::id(),
            NEXT_JOB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).map_err(|e| format!("could not reserve bounce folder: {e}"))?;
        let paths = (0..plan.len())
            .map(|i| folder.join(format!("track-{i}.wav")))
            .collect::<Vec<_>>();
        let mut published = Vec::new();
        let result = (|| {
            let mut prepared = Vec::new();
            let mut files = Vec::new();
            for (item, path) in plan.iter().zip(&paths) {
                if control.is_cancelled() || self.shutdown.load(Ordering::Acquire) {
                    return Err("bounce cancelled".into());
                }
                let tails = target.includes_tails();
                let maximum = (item.end - item.start)
                    .checked_add(if tails {
                        u64::from(self.project().sample_rate())
                            .checked_mul(hex_export::MAX_ARRANGEMENT_BOUNCE_TAIL_SECONDS)
                            .ok_or("bounce tail budget overflows")?
                    } else {
                        0
                    })
                    .filter(|n| *n <= hex_project::MAX_ARRANGEMENT_BOUNCE_FRAMES)
                    .ok_or("bounce including tail allowance exceeds its frame budget")?;
                let retained = prepared
                    .iter()
                    .map(|p: &PreparedImport| p.waveform.source_frames as u64)
                    .sum::<u64>();
                if self
                    .resident_frames
                    .checked_add(retained)
                    .and_then(|n| n.checked_add(maximum))
                    .and_then(|n| {
                        self.native_assets
                            .capacity_frames()
                            .ok()
                            .and_then(|a| a.checked_add(n))
                    })
                    .is_none_or(|n| n > self.max_resident_frames)
                {
                    return Err("bounce exceeds resident frame limit".into());
                }
                let source = isolated(self.project(), &target, item, in_place)?;
                let revision = prepare(
                    self,
                    &source,
                    maximum
                        .checked_add(retained)
                        .ok_or("bounce allowance overflows")?,
                )?;
                let frames = render(path, revision, item, tails, control)?;
                published.push(path.clone());
                let decoded = hex_media::decode_wav_file(
                    path,
                    hex_media::Limits::new(
                        64 * 1024 * 1024 + 44,
                        hex_project::MAX_ARRANGEMENT_BOUNCE_FRAMES as usize,
                    ),
                )
                .map_err(|e| e.to_string())?;
                prepared.push(PreparedImport::new(decoded).map_err(|e| e.to_string())?);
                files.push(BounceFile {
                    source: item.source,
                    track: item.track,
                    start: item.start,
                    frames,
                    path: path.clone(),
                });
            }
            if control.is_cancelled() || self.shutdown.load(Ordering::Acquire) {
                return Err("bounce cancelled".into());
            }
            commit(self, prepared, files)
        })();
        if result.is_err() {
            for path in published {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir(folder);
        }
        result
    }

    /// Reserved complete media adoption. The caller's last preparation hook
    /// rechecks cancellation and seals/enqueues only as its last fallible action.
    #[allow(clippy::too_many_arguments)] // One captured adoption intent per field.
    pub(crate) fn adopt_arrangement_bounce_using<T>(
        &mut self,
        target: BounceTarget,
        in_place: bool,
        adoption: hex_project::BounceAdoption,
        files: Vec<BounceFile>,
        prepared: Vec<PreparedImport>,
        previous: Option<&hex_engine::PreparedRevision>,
        prepare: impl FnOnce(import_batch::Preparation<'_>) -> Result<T, SessionError>,
    ) -> Result<(Created, T), SessionError> {
        if prepared.len() != files.len() || prepared.is_empty() {
            return Err(SessionError::Invalid("bounce media count differs"));
        }
        self.media
            .try_reserve(prepared.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut residents = Vec::new();
        residents
            .try_reserve_exact(prepared.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut total_frames = self.resident_frames;
        let mut total_bins = self.resident_waveform_bins;
        let mut total_bytes = self.resident_waveform_bytes;
        for (member, file) in prepared.into_iter().zip(&files) {
            if member.audio.sample_rate() != self.project().sample_rate()
                || member.waveform.source_frames as u64 != file.frames
                || member.path != file.path
            {
                return Err(SessionError::Invalid(
                    "bounce decoded media differs from output",
                ));
            }
            total_frames = total_frames
                .checked_add(
                    member.additional_pcm_frames(
                        self.media
                            .values()
                            .chain(residents.iter())
                            .flat_map(|r: &ResidentAsset| [&r.source_audio, &r.audio]),
                    )?,
                )
                .filter(|n| {
                    self.native_assets
                        .capacity_frames()
                        .ok()
                        .and_then(|a| a.checked_add(*n))
                        .is_some_and(|n| n <= self.max_resident_frames)
                })
                .ok_or(SessionError::Invalid("bounce resident frame limit reached"))?;
            self.validate_native_legacy_budget(total_frames)?;
            let bins = member.waveform.cached_bin_count();
            total_bins = total_bins
                .checked_add(bins)
                .ok_or(SessionError::Invalid("bounce waveform count overflows"))?;
            total_bytes = total_bytes
                .checked_add(
                    bins.checked_mul(mem::size_of::<[hex_media::waveform::Peak; 2]>())
                        .ok_or(SessionError::Allocation)?,
                )
                .ok_or(SessionError::Allocation)?;
            residents.push(ResidentAsset {
                source_audio: member.source_audio,
                audio: member.audio,
                waveform: Arc::new(member.waveform),
                frames: file.frames,
                file_identity: member.file_identity,
            });
        }
        let mut media = self.media_bank();
        let waveforms = std::cell::RefCell::new(self.audio_clip_waveforms()?);
        let legacy = self
            .media
            .values()
            .chain(residents.iter())
            .flat_map(|r| [&r.source_audio, &r.audio])
            .collect::<Vec<_>>();
        let table = self.sine_table();
        let native = &self.native_assets;
        let transforms = &self.audio_transforms;
        let limits = self.native_reservation;
        let max_frames = self.max_resident_frames;
        let shutdown = &self.shutdown;
        let (created, (ids, artifact)) = self.editor.execute_prepared(
            Command::CommitArrangementBounce {
                target,
                in_place,
                adoption,
                files,
            },
            |candidate| {
                let mut ids = candidate
                    .assets()
                    .filter(|a| !media.contains_key(&a.id()))
                    .map(|a| a.id())
                    .collect::<Vec<_>>();
                ids.sort();
                if ids.len() != residents.len() {
                    return Err(SessionError::Invalid(
                        "bounce candidate asset count differs",
                    ));
                }
                for (id, r) in ids.iter().zip(&residents) {
                    media.insert(*id, r.audio.clone());
                }
                let artifact = prepare(import_batch::Preparation {
                    project: candidate,
                    media: &media,
                    native,
                    table,
                    transforms,
                    waveforms: &waveforms,
                    legacy: &legacy,
                    limits,
                    max_frames,
                    shutdown,
                    previous,
                })?;
                Ok::<_, SessionError>((ids, artifact))
            },
        )?;
        *self.audio_clip_waveforms.get_mut() = waveforms.into_inner();
        for (id, r) in ids.into_iter().zip(residents) {
            self.media.insert(id, r);
        }
        self.resident_frames = total_frames;
        self.resident_waveform_bins = total_bins;
        self.resident_waveform_bytes = total_bytes;
        Ok((created, artifact))
    }
}

impl Session {
    /// Headless control entry point; native workers use the same transaction
    /// with their existing device-resource admission/publication hook.
    pub fn bounce_arrangement(
        &mut self,
        target: BounceTarget,
        in_place: bool,
        directory: &Path,
        control: &hex_export::ExportControl,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), String> {
        self.bounce_arrangement_adopting(
            target,
            in_place,
            hex_project::BounceAdoption::NewTracks,
            directory,
            control,
            previous,
        )
    }

    /// As [`Self::bounce_arrangement`] with an explicit new-track adoption:
    /// optionally muting the sources or landing at a clip drop, in the same
    /// adoption edit and Undo step.
    pub fn bounce_arrangement_adopting(
        &mut self,
        target: BounceTarget,
        in_place: bool,
        adoption: hex_project::BounceAdoption,
        directory: &Path,
        control: &hex_export::ExportControl,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), String> {
        self.bounce_arrangement_adopting_with(
            target, in_place, adoption, directory, control, previous, None,
        )
    }

    /// As [`Self::bounce_arrangement_adopting`]; items with file-backed sources
    /// render through `streamed`, the host's offline page provider. Without one
    /// they refuse explicitly before adoption.
    #[allow(clippy::too_many_arguments)] // The adopting contract plus one host capability.
    pub fn bounce_arrangement_adopting_with(
        &mut self,
        target: BounceTarget,
        in_place: bool,
        adoption: hex_project::BounceAdoption,
        directory: &Path,
        control: &hex_export::ExportControl,
        previous: Option<&hex_engine::PreparedRevision>,
        streamed: Option<StreamedBouncer>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), String> {
        let captured = target.clone();
        self.bounce_arrangement_using(
            target,
            in_place,
            directory,
            control,
            |session, project, _| {
                let waveforms = std::cell::RefCell::new(
                    session.audio_clip_waveforms().map_err(|e| e.to_string())?,
                );
                crate::prepare_with_waveforms(
                    project,
                    &session.media_bank(),
                    session.sine_table(),
                    None,
                    &session.audio_transforms,
                    &waveforms,
                )
                .map(|prepared| {
                    streamed::BounceRevision::capture(Arc::new(prepared), &session.verified_sources)
                })
                .map_err(|e| e.to_string())
            },
            |path, revision, item, tails, control| {
                streamed::render(
                    path,
                    revision,
                    None,
                    item.start,
                    item.end - item.start,
                    tails,
                    control,
                    streamed,
                )
            },
            |session, prepared, files| {
                session
                    .adopt_arrangement_bounce_using(
                        captured,
                        in_place,
                        adoption,
                        files,
                        prepared,
                        previous,
                        |input| {
                            crate::import_batch::prepare_asset_free(input, &mut || {
                                control.seal_prepared_adoption().map_err(|_| {
                                    SessionError::Invalid("bounce cancelled before adoption")
                                })
                            })
                        },
                    )
                    .map_err(|e| e.to_string())
            },
        )
    }
}

#[cfg(test)]
mod tests;
