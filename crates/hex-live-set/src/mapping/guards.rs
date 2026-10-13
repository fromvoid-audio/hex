use super::*;

pub(super) fn guard_track(t: Node<'_, '_>, main: bool, tempo: f64) -> Result<(), Error> {
    same(t, "TrackDelay/Value", "0", "track delay")?;
    for field in child(t, "TrackDelay")?
        .children()
        .filter(|n| n.is_element())
    {
        if !["Value", "IsValueSampleBased"].contains(&field.tag_name().name()) {
            return Err(Error::Unsupported("track timing law"));
        }
    }
    same(t, "TrackGroupId", "-1", "track group")?;
    empty(t, "TakeLanes/TakeLanes", "take lanes")?;
    empty(t, "DeviceChain/DeviceChain/Devices", "devices or plugins")?;
    empty(t, "DeviceChain/DeviceChain/SignalModulations", "modulation")?;
    for field in child(t, "DeviceChain")?
        .children()
        .filter(|n| n.is_element())
    {
        if ![
            "AutomationLanes",
            "ClipEnvelopeChooserViewState",
            "AudioInputRouting",
            "MidiInputRouting",
            "AudioOutputRouting",
            "MidiOutputRouting",
            "Mixer",
            "MainSequencer",
            "FreezeSequencer",
            "DeviceChain",
        ]
        .contains(&field.tag_name().name())
        {
            return Err(Error::Unsupported("unknown signal-chain behavior"));
        }
    }
    for field in at(t, "DeviceChain/DeviceChain")?
        .children()
        .filter(|n| n.is_element())
    {
        if !["Devices", "SignalModulations"].contains(&field.tag_name().name()) {
            return Err(Error::Unsupported("unknown device-chain behavior"));
        }
    }
    let m = at(t, "DeviceChain/Mixer")?;
    empty(m, "Sends", "sends")?;
    same(m, "On/Manual", "true", "mixer bypass")?;
    same(m, "SoloSink", "false", "solo")?;
    const MIXER_FIELDS: &[&str] = &[
        "LomId",
        "LomIdView",
        "IsExpanded",
        "BreakoutIsExpanded",
        "On",
        "ModulationSourceCount",
        "ParametersListWrapper",
        "Pointee",
        "LastSelectedTimeableIndex",
        "LastSelectedClipEnvelopeIndex",
        "LastPresetRef",
        "LockedScripts",
        "IsFolded",
        "ShouldShowPresetName",
        "UserName",
        "Annotation",
        "SourceContext",
        "MpePitchBendUsesTuning",
        "ViewData",
        "Sends",
        "Speaker",
        "SoloSink",
        "PanMode",
        "Pan",
        "SplitStereoPanL",
        "SplitStereoPanR",
        "Volume",
        "ViewStateSessionTrackWidth",
        "CrossFadeState",
        "SendsListWrapper",
        "Tempo",
        "TimeSignature",
        "GlobalGrooveAmount",
        "CrossFade",
        "TempoAutomationViewBottom",
        "TempoAutomationViewTop",
        "UserTempoAutomation",
    ];
    for field in m.children().filter(|n| n.is_element()) {
        if !MIXER_FIELDS.contains(&field.tag_name().name()) {
            return Err(Error::Unsupported("unknown mixer behavior"));
        }
    }
    same(m, "ModulationSourceCount", "0", "mixer modulation")?;
    same(m, "PanMode", "0", "split stereo pan")?;
    same(m, "CrossFadeState/Manual", "1", "crossfade assignment")?;
    if t.has_tag_name("AudioTrack") || t.has_tag_name("MidiTrack") {
        for sequencer in ["MainSequencer", "FreezeSequencer"] {
            let path = format!("DeviceChain/{sequencer}/ClipSlotList");
            for slot in at(t, &path)?.children().filter(|n| n.is_element()) {
                if !slot.has_tag_name("ClipSlot") {
                    return Err(Error::Unsupported("session container"));
                }
                empty(slot, "ClipSlot/Value", "session content")?;
            }
        }
        same(t, "Freeze", "false", "frozen track")?;
        same(t, "PostProcessFreezeClips", "0", "freeze processing")?;
    }
    let envelopes = at(t, "AutomationEnvelopes/Envelopes")?;
    if !main {
        return empty(t, "AutomationEnvelopes/Envelopes", "automation");
    }
    // This fixture's initial meter/tempo are negative-time sentinel envelopes,
    // not time-varying automation. Only these two constant initializers are supported.
    let tempo_id = attr(at(m, "Tempo/AutomationTarget")?, "Id")?;
    let meter_id = attr(at(m, "TimeSignature/AutomationTarget")?, "Id")?;
    let mut found = Vec::new();
    for envelope in envelopes.children().filter(|n| n.is_element()) {
        if !envelope.has_tag_name("AutomationEnvelope") {
            return Err(Error::Unsupported("main automation"));
        }
        let target = value(envelope, "EnvelopeTarget/PointeeId")?;
        if found.contains(&target) {
            return Err(Error::Invalid("duplicate main envelope"));
        }
        found.push(target);
        let events: Vec<_> = at(envelope, "Automation/Events")?
            .children()
            .filter(|n| n.is_element())
            .collect();
        if events.len() != 1 {
            return Err(Error::Unsupported("time-varying automation"));
        }
        let e = events[0];
        if attr(e, "Time")? != "-63072000" {
            return Err(Error::Unsupported("automation timing"));
        }
        if target == tempo_id && e.has_tag_name("FloatEvent") {
            if numeric(attr(e, "Value")?)? != tempo {
                return Err(Error::Unsupported("tempo envelope mismatch"));
            }
        } else if target == meter_id && e.has_tag_name("EnumEvent") {
            if attr(e, "Value")? != "201" {
                return Err(Error::Unsupported("meter envelope"));
            }
        } else {
            return Err(Error::Unsupported("main automation"));
        }
    }
    if found.len() != 2 {
        return Err(Error::Unsupported(
            "missing initial tempo or meter envelope",
        ));
    }
    Ok(())
}
pub(super) fn guard_clip(c: Node<'_, '_>, audio: bool) -> Result<(), Error> {
    same(c, "Loop/LoopOn", "false", "clip loop")?;
    same(c, "Loop/StartRelative", "0", "source-relative start")?;
    same(c, "Disabled", "false", "disabled clip")?;
    same(c, "FreezeStart", "0", "clip freeze")?;
    same(c, "FreezeEnd", "0", "clip freeze")?;
    same(c, "GrooveSettings/GrooveId", "-1", "clip groove")?;
    same(c, "VelocityAmount", "0", "velocity modulation")?;
    empty(c, "Envelopes/Envelopes", "clip envelopes")?;
    if child(c, "TimeSignature")?
        .children()
        .filter(|n| n.is_element())
        .count()
        != 1
    {
        return Err(Error::Unsupported("unknown clip meter law"));
    }
    let signatures = at(c, "TimeSignature/TimeSignatures")?;
    let sigs: Vec<_> = signatures.children().filter(|n| n.is_element()).collect();
    if sigs.len() != 1 {
        return Err(Error::Unsupported("clip time signatures"));
    }
    same(sigs[0], "Numerator", "4", "clip meter")?;
    same(sigs[0], "Denominator", "4", "clip meter")?;
    same(sigs[0], "Time", "0", "clip meter timing")?;
    // These direct fields are observed in the authentic fixture. Unknown direct
    // clip fields are conservatively refused; annotations/view data remain opaque.
    const COMMON: &[&str] = &[
        "LomId",
        "LomIdView",
        "CurrentStart",
        "CurrentEnd",
        "Loop",
        "Name",
        "Annotation",
        "Color",
        "LaunchMode",
        "LaunchQuantisation",
        "TimeSignature",
        "Envelopes",
        "ScrollerTimePreserver",
        "TimeSelection",
        "Legato",
        "Ram",
        "GrooveSettings",
        "Disabled",
        "VelocityAmount",
        "FollowAction",
        "Grid",
        "FreezeStart",
        "FreezeEnd",
        "IsWarped",
        "TakeId",
        "IsInKey",
        "ScaleInformation",
        "AutomationEnvelopesListWrapper",
    ];
    const AUDIO: &[&str] = &[
        "SampleRef",
        "Onsets",
        "WarpMode",
        "GranularityTones",
        "GranularityTexture",
        "FluctuationTexture",
        "TransientResolution",
        "TransientLoopMode",
        "TransientEnvelope",
        "ComplexProFormants",
        "ComplexProEnvelope",
        "Sync",
        "HiQ",
        "Fade",
        "Fades",
        "PitchCoarse",
        "PitchFine",
        "SampleVolume",
        "WarpMarkers",
        "SavedWarpMarkersForStretched",
        "MarkersGenerated",
        "IsSongTempoLeader",
    ];
    const MIDI: &[&str] = &[
        "Notes",
        "BankSelectCoarse",
        "BankSelectFine",
        "ProgramChange",
        "NoteEditorFoldInZoom",
        "NoteEditorFoldInScroll",
        "NoteEditorFoldOutZoom",
        "NoteEditorFoldOutScroll",
        "NoteEditorFoldScaleZoom",
        "NoteEditorFoldScaleScroll",
        "NoteSpellingPreference",
        "AccidentalSpellingPreference",
        "PreferFlatRootNote",
        "ExpressionGrid",
    ];
    for field in c.children().filter(|n| n.is_element()) {
        let name = field.tag_name().name();
        if !COMMON.contains(&name) && !(if audio { AUDIO } else { MIDI }).contains(&name) {
            return Err(Error::Unsupported("unknown clip behavior"));
        }
    }
    if !audio {
        for field in child(c, "Notes")?.children().filter(|n| n.is_element()) {
            if ![
                "KeyTracks",
                "PerNoteEventStore",
                "NoteProbabilityGroups",
                "ProbabilityGroupIdGenerator",
                "NoteIdGenerator",
            ]
            .contains(&field.tag_name().name())
            {
                return Err(Error::Unsupported("unknown note behavior"));
            }
        }
        empty(
            c,
            "Notes/PerNoteEventStore/EventLists",
            "per-note expression",
        )?;
        empty(c, "Notes/NoteProbabilityGroups", "note probability")?;
        for path in ["BankSelectCoarse", "BankSelectFine", "ProgramChange"] {
            same(c, path, "-1", "midi program or bank")?;
        }
    }
    Ok(())
}
