from pathlib import Path
p=Path('crates/hex-ui/src/editor/track_gain.rs');s=p.read_text();s=s.replace('use hex_project::{Track, TrackId};','use hex_project::{ReturnId, ReturnTrack, Track, TrackId};');s=s.replace('    bindings: BTreeMap<TrackId, Binding>,','    bindings: BTreeMap<TrackId, Binding>,\n    mixer: BTreeMap<MixerKey, Binding>,');idx=s.index('struct Binding {');s=s[:idx]+'''#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MixerKey {
    Balance(TrackId),
    ReturnGain(ReturnId),
}
impl MixerKey {
    fn target(self) -> GestureTarget {
        match self {
            Self::Balance(track) => GestureTarget::TrackBalance { track },
            Self::ReturnGain(return_track) => GestureTarget::ReturnGain { return_track },
        }
    }
    fn current(self, view: &EditorView) -> bool {
        view.state.as_ref().is_some_and(|state| match self {
            Self::Balance(track) => state.project.track(track).is_some(),
            Self::ReturnGain(id) => state.project.return_tracks().iter().any(|track| track.id() == id),
        })
    }
}

'''+s[idx:]
idx=s.index('    view.track_gain_controls.generation = view.project_generation;');s=s[:idx]+'''    let stale = view.track_gain_controls.mixer.keys().copied().filter(|key| {
        view.track_gain_controls.generation != view.project_generation || !key.current(view)
    }).collect::<Vec<_>>();
    for key in stale {
        if let Some(binding) = view.track_gain_controls.mixer.remove(&key) {
            if device_panel::owns_numeric_gesture(view, key.target(), &binding.lease) {
                let _ = view.cancel_device_gesture();
            }
            binding.lease.set(None);
            binding.entity.update(cx, |control, cx| control.cancel(cx));
        }
    }
'''+s[idx:]
a=s.index('    view.track_gain_controls\n        .bindings',s.index('pub(super) fn controls_for_finalize'));b=s.index('\n}\n\nimpl EditorView',a);seg=s[a:b];seg=seg.replace('        .collect()','        .chain(view.track_gain_controls.mixer.iter().map(|(key, binding)| (key.target(), binding.entity.clone())))\n        .collect()');s=s[:a]+seg+s[b:];s=s.replace('.min_w(px(86.0))','.min_w(px(60.0))').replace('.h(px(21.0))','.h(px(18.0))')
s+='''
impl EditorView {
    pub(super) fn track_pan(&mut self, track: &Track, cx: &mut Context<Self>) -> Div {
        let Ok(spec) = NumericSpec::new(-1.0, 1.0, 0.0, NumericScale::Bipolar, 2, None::<&str>) else { return div() };
        self.mixer_control(MixerKey::Balance(track.id()), format!("track-pan-{}", track.id().value()),
            "track pan · original stereo balance", device_panel::balance_spec(spec), track.balance(), cx)
    }

    pub(super) fn return_volume(&mut self, track: &ReturnTrack, cx: &mut Context<Self>) -> Div {
        let Ok(spec) = NumericSpec::new(0.0, 2.0, 1.0, NumericScale::LinearAmplitudeDecibels, 1, Some("db")) else { return div() };
        self.mixer_control(MixerKey::ReturnGain(track.id()), format!("return-volume-{}", track.id().value()),
            "return volume · db", spec, track.gain(), cx)
    }

    fn mixer_control(&mut self, key: MixerKey, id: String, label: &str, spec: NumericSpec,
        canonical: f32, cx: &mut Context<Self>) -> Div {
        retire_stale(self, cx);
        let target = key.target();
        let generation = self.project_generation;
        if let std::collections::btree_map::Entry::Vacant(entry) = self.track_gain_controls.mixer.entry(key) {
            let Ok(config) = NumericControlConfig::new(id, theme::text(label), NumericControlKind::HorizontalSlider,
                spec, f64::from(canonical), device_panel::control_style()) else { return div() };
            let entity = cx.new(|cx| NumericControl::from_config(cx, config.with_appearance(NumericControlAppearance::FilledSlider)));
            let lease: device_panel::NumericGestureLease = Rc::new(Cell::new(None));
            let captured = lease.clone();
            let subscription = cx.subscribe(&entity, move |view, control, event: &NumericControlEvent, cx| {
                if generation != view.project_generation || !key.current(view) { return; }
                if event.phase == NumericEventPhase::Begin && (view.document_audio_mode != hex_session::DocumentAudioMode::Resident
                    || view.gain_recorder.blocks_edits() || view.edit_pending()) {
                    view.error = Some("finish the current edit · held mixer controls require resident audio".into());
                    control.update(cx, |control, cx| control.cancel(cx));
                    cx.notify();
                    return;
                }
                if let Err(reason) = device_panel::dispatch_bound_numeric_gesture(view, target, &captured, event.phase,
                    ParameterValue::Float(event.value as f32)) {
                    view.error = Some(reason.into());
                    control.update(cx, |control, cx| control.cancel(cx));
                }
                cx.notify();
            });
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            entry.insert(Binding { entity, lease, _subscription: subscription, _observation: observation });
        }
        let active = self.device_gesture.snapshot().filter(|active| active.target == target).and_then(|active| match active.value {
            ParameterValue::Float(value) => Some(value), _ => None,
        });
        let enabled = self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && !self.gain_recorder.blocks_edits() && !self.opening_project && !self.close_prompt_open;
        let binding = &self.track_gain_controls.mixer[&key];
        binding.entity.update(cx, |control, cx| {
            control.set_enabled(enabled, cx);
            let _ = control.set_external_style(device_panel::control_style(), cx);
            let _ = control.set_external_value(f64::from(active.unwrap_or(canonical)), cx);
        });
        div().flex_1().min_w(px(60.0)).h(px(18.0)).child(binding.entity.clone())
    }
}
''';p.write_text(s)
p=Path('crates/hex-ui/src/editor/device_panel.rs');s=p.read_text();idx=s.index('pub(super) fn device_layout_style()');s=s[:idx]+'''// Reuse the canonical device-side balance display/parser for track mixer fields.
pub(super) fn balance_spec(spec: NumericSpec) -> NumericSpec {
    numeric_display::NumericDisplayCodec::Balance.apply(spec)
}

'''+s[idx:];p.write_text(s)
