# PR439 / Pulse2835 supported header-mixer component

Source f44ca6ff053ed3106622cf0e2885b14db3d30a64, tree5c04c01ea940cd3a618c6c42e74ef623e13f4012.
Same canonical pulse/task-2835-g1-decc6ba9bc1a / PR439, original plan ce38ef26 preserved.
One ordinary main9d1d0f0bbd14fcbc2ff0eecb975f52db65a12aa2 integration gives before fb4cb8be3896b73c8ebda76027bbcbfbd02622ec.
Owner commits6953c416 + f44ca6ff, no force-push, no separate PR.

Resident headers use a compact two-row122×38 mixer beside name/automation/send
controls, within the unchanged176×44 minimum. Actual activator, Solo and gain/pan
retain canonical commands; arm is clearly disabled/unavailable. FilledSlider is
an additive shared appearance with centered click-to-type text/horizontal drag,
existing dB range/silence and bipolar l/r/c codec. Standard/CompactSlider geometry,
423/416 text paths, other device controls, automation-owned auto and streaming
explicit +/- stay intact. Return gain/mute share the applicable appearance and
remain pinned; no Return-pan/Solo model is invented.

Root-approved additive TrackBalance and ReturnGain targets use the existing
worker token/generation/sequence/rebase/Cancel/terminal ACK/install path. The
private finite preview_mix_command extraction preserves existing gain preparation;
new commands are existing Balance/ReturnGain only. No Engine/Audio/model/storage
schema, recording implementation, new owner/queue or unmerged438/431 import.

Final SAME48270 TERMINAL0,13/13PASS154.848s, clean source/start=end, stale=false.
111 unique tests,0ignored:11Session track_gain_gesture (real stereo/bus PCM,
canonical preview/commit/Cancel/UndoRedo/current SaveOpen/nonfinite/removed owner/
streaming refusal plus old coalescing/rebase/history),6existingReturn lifecycle,
9actualheader UI/Service,5track-layout,80sharedwidgets. Strict Sessionalltargets,
UI+widgetsdefault and UIfeaturealltargets, format/architecture/UI-style/vendor/
diff guards passed. Source content SHA65215c8b9f876b3e6228a3aec17fa44f391407b46a01c53363a8ce245940f07b.
Final summary SHAddf1b0a8ab74662f54785d48ee5cf6c293facc23206c18b528582a33aa9aa571.
This is component evidence, NOT prospective full21 qualification/merge/native delivery.
Existing build-script warning about PCM Arc layout Unknown remains visible in raw logs;
no new Rust lint warning/error is waived.

Original actual before UI RED:86×21root with separate46×18value,19px center offset.
Its exact initially dirty test bytes were recovered and match the ENTIRE original
repository content digest9ad91909f9160128a242c129c3d67ec8a987459d6d123a7f11dec17dfa5715f3.
The proof uses actual unchanged workspace executable bits (tar Git modes alone
first mismatched); this is source recovery, not another execution/relabeling.
Raw earlier diagnostics retained: wrong test method E0599; focus-inset expectation;
missing exhaustive arms/Div type/mutable Return view compiler errors; no-op pan
fixture missing Playback; actual nested6px bar/drag RED repaired with flex sizing;
new test RemoveTrack tuple/compiler and keyed Div click/compiler errors. The clean
6953 precursor passed all111 functional tests/strict/architecture but UI-style
stopped at8new typed children; final registrations are exact and file-local.
Nothing is skipped, ignored, silently retried for randomness, or reclassified as
native evidence. Old test oracles retained except the explicitly requested rail→
combined-field geometry/2D nonoverlap contract.

visual/ includes unchanged original user reference PNG21310B/SHA
c8ac2e8ef899ae845eaac0dd6003f3a37594a84094afbe5eb90cce2080a352eb,
actual production completed-scene bounds/quad JSON and a clearly labeled observed
subset reconstruction. Captions in that chart are canonical fixture values, not
captured glyphs/native pixels. Linux test shaping is no-op; the same reference
case opts into native bundled Ioskeley on Mac. Font/OFL supplied for artifact
viewing, no proprietary artwork copied into production.

FULL2835 remains open: functional recording arm is deferred Pulse26/version0.2;
PR190 is closed UNMERGED and is not resurrected. Return pan/Solo are absent from
the current model and omitted. Source-ready component does not satisfy those
requirements or native visual/listening acceptance. Preserve user-rejected2715
state/history. Root owns later exact optimized comparison/listening/checklist in
docs/track-header-volume.md. No app/native/build/install/launch/quit actions occurred.

Merger: compose ONLY the16-file delta from fb4cb8be/main9d; keep other canonical
423/416/447/438/431 work by precise hunks, not whole-file overwrite. Final current
prospective full21/fresh review/merge is yours. Pulse remains InProgress until
actual integration; do not infer all acceptance or mark Done. Public target enum
has two finite additions with all exhaustive UI consumers updated; no native
registry/resource/DSP edits. Ready for a concrete review return if any.

Queued next: same459 review34877501 null-owner/directory identity, after this freeze;
no duplicate worker, no native work or wait for439 merge.
