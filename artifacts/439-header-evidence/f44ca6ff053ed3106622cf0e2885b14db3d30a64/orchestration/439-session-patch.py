from pathlib import Path
p=Path('crates/hex-session/src/lib.rs');s=p.read_text();a=s.index('        let mut preview = Editor::new(self.project().clone(), 1)?;',s.index('    pub fn preview_track_gain('));b=s.index('    /// Publish the final absolute',a);old=s[a:b];new='''        self.preview_mix_command(Command::Gain { track, gain }, previous)
    }

    // Finite mixer callers only. This does not admit arbitrary UI commands.
    fn preview_mix_command(
        &self,
        command: Command,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
'''+old.replace('preview.execute_prepared(Command::Gain { track, gain }, |candidate| {','preview.execute_prepared(command, |candidate| {');s=s[:a]+new+s[b:];idx=s.index('    /// Prepare a sparse return-send overlay')
s=s[:idx]+'''    fn preview_track_balance(
        &self,
        track: hex_project::TrackId,
        balance: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.preview_mix_command(Command::Balance { track, balance }, previous)
    }

    fn commit_track_balance(
        &mut self,
        track: hex_project::TrackId,
        balance: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        self.command_prepared_revision(Command::Balance { track, balance }, previous)
            .map(|(_, prepared)| prepared)
    }

    fn preview_return_gain(
        &self,
        return_track: hex_project::ReturnId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.preview_mix_command(Command::ReturnGain { return_track, gain }, previous)
    }

    fn commit_return_gain(
        &mut self,
        return_track: hex_project::ReturnId,
        gain: f32,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        self.command_prepared_revision(Command::ReturnGain { return_track, gain }, previous)
            .map(|(_, prepared)| prepared)
    }

'''+s[idx:];p.write_text(s)
p=Path('crates/hex-session/src/device_gesture.rs');s=p.read_text();s=s.replace('    DeviceParameter {\n','    TrackBalance {\n        track: TrackId,\n    },\n    ReturnGain {\n        return_track: ReturnId,\n    },\n    DeviceParameter {\n',1);s=s.replace('    TrackGain,\n','    TrackGain,\n    TrackBalance,\n    ReturnGain,\n',1)
needle='                    GestureTarget::TrackGain { track } => (';pos=s.index(needle);s=s[:pos]+'''                    GestureTarget::TrackBalance { track } => (
                        ActiveKind::TrackBalance,
                        ParameterValue::Float(static_track_balance(session, track)?),
                        None,
                    ),
                    GestureTarget::ReturnGain { return_track } => (
                        ActiveKind::ReturnGain,
                        ParameterValue::Float(static_return_gain(session, return_track)?),
                        None,
                    ),
'''+s[pos:]
pos=s.index('            (GestureTarget::TrackGain { track }, ActiveKind::TrackGain) => {');s=s[:pos]+'''            (GestureTarget::TrackBalance { track }, ActiveKind::TrackBalance) => {
                static_track_balance(session, track)?;
            }
            (GestureTarget::ReturnGain { return_track }, ActiveKind::ReturnGain) => {
                static_return_gain(session, return_track)?;
            }
'''+s[pos:]
pos=s.index('        GestureTarget::TrackGain { track } => {',s.index('fn preview_target('));s=s[:pos]+'''        GestureTarget::TrackBalance { track } => {
            session.preview_track_balance(track, mix_value(value, -1.0, 1.0)?, previous)
        }
        GestureTarget::ReturnGain { return_track } => {
            session.preview_return_gain(return_track, mix_value(value, 0.0, 2.0)?, previous)
        }
'''+s[pos:]
pos=s.index('        GestureTarget::TrackGain { track } => {',s.index('fn commit_target('));s=s[:pos]+'''        GestureTarget::TrackBalance { track } => {
            session.commit_track_balance(track, mix_value(value, -1.0, 1.0)?, previous)
        }
        GestureTarget::ReturnGain { return_track } => {
            session.commit_return_gain(return_track, mix_value(value, 0.0, 2.0)?, previous)
        }
'''+s[pos:]
pos=s.index('fn static_track_gain(');s=s[:pos]+'''fn static_track_balance(session: &Session, track: TrackId) -> Result<f32, SessionError> {
    require_resident_mix(session)?;
    session.project().track(track).map(hex_project::Track::balance)
        .ok_or(SessionError::Invalid("track balance target no longer exists"))
}

fn static_return_gain(session: &Session, id: ReturnId) -> Result<f32, SessionError> {
    require_resident_mix(session)?;
    session.project().return_tracks().iter().find(|track| track.id() == id)
        .map(hex_project::ReturnTrack::gain)
        .ok_or(SessionError::Invalid("return gain target no longer exists"))
}

fn require_resident_mix(session: &Session) -> Result<(), SessionError> {
    if session.audio_mode() != crate::DocumentAudioMode::Resident {
        return Err(SessionError::Invalid("held mixer controls require resident audio mode"));
    }
    Ok(())
}

fn mix_value(value: ParameterValue, min: f32, max: f32) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(value) if value.is_finite() && (min..=max).contains(&value) => Ok(value),
        _ => Err(SessionError::Invalid("mixer value must be finite and within its range")),
    }
}

'''+s[pos:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/device_panel/gesture_adapter.rs');s=p.read_text();pos=s.index('            GestureTarget::TrackGain { track } =>');s=s[:pos]+'''            GestureTarget::TrackBalance { track } => Request::Edit(Command::Balance {
                track,
                balance: match value {
                    ParameterValue::Float(balance) => balance,
                    _ => return Err("track balance requires a float value"),
                },
            }),
            GestureTarget::ReturnGain { return_track } => Request::Edit(Command::ReturnGain {
                return_track,
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("return gain requires a float value"),
                },
            }),
'''+s[pos:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/device_panel.rs');s=p.read_text().replace('GestureTarget::ReturnSend { .. } | GestureTarget::TrackGain { .. } => false,','GestureTarget::ReturnSend { .. } | GestureTarget::TrackGain { .. }\n        | GestureTarget::TrackBalance { .. } | GestureTarget::ReturnGain { .. } => false,');p.write_text(s)
p=Path('crates/hex-widgets/src/numeric_control/compact_slider_tests.rs');s=p.read_text().replace('assert_eq!(bar.size.width, px(width));','assert_eq!(bar.size.width, px(width - 2.0), "retain shared focus-marker inset");');p.write_text(s)
