# Native Live fixture provenance — 11 October 2026

Completed a genuine Live-authored minimal set in Ableton Live 12.4.6. The compressed ALS bytes were saved through Live and never rewritten. `observed-values.json` is the authoritative source-inspected inventory and hash manifest; the earlier planned values are superseded.

## Native actions

Saved a separate blank set; retained one track while deleting extras/returns; removed the default groove. Renamed audio track, imported original generated WAV, disabled warp/loop/fades, set source markers 0.5–2.5 seconds, and trimmed Arrangement placement to beats 4–8. Set audio gain -6 dB and pan -0.25. Created a genuine MIDI track; removed its inherited user-default device chain; imported original two-note SMF at beats 12–16 without importing tempo/meter; disabled clip looping and muted the track. Used Collect All and Save. Final source confirms AudioTrack then MidiTrack, no devices, one original-media FileRef. An earlier WAV import converted the MIDI track; that track was replaced before this final save.

## Original inputs and checks

WAV: 4 seconds, stereo PCM16, 48 kHz. At frame i modulo 4800, left is 8192 at 0, -4096 at 1, otherwise 0; right is -6144 at 2, 2048 at 3, otherwise 0. Header and all 192000 PCM frames checked, and collected WAV bytes match original. SMF supplies note 60 at beat 0 for 1 beat, velocity 64; note 67 at beat 1.5 for 0.5 beat, velocity 100. The final ALS note inventory matches these values. Arrangement quarter-note beats are distinct from unwarped audio source seconds.

## Package boundary and remaining acceptance

Only final authentic ALS, referenced original WAV, original SMF authoring input, and provenance are packaged. No backups, factory content, default-device sample, BINDING7, analysis cache or unreferenced preview audio is included. The ALS retains its native absolute path for the original collected WAV and a valid relative path. No production set or user default was overwritten. A fixture folder was added to Live browser Places during authoring. Live was left stopped with audio engine off. Native control was explicitly released to the other evidence owners.

Hex import/render/round-trip and audible acceptance remain with the sole cloud adapter coordinator. This fixture proves saved source structure and authoring provenance, not adapter behavior or Live parity. Actual MIDI mixer metadata is inherited -12 dB/neutral pan, rather than the earlier planned 0 dB/+0.5; observed values take precedence.
