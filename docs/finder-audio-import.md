# Finder audio import — pulse2385 / pr172

The local OS-drop adapter connects GPUI ExternalPaths to the existing canonical
bounded audio import worker. The platform retains file paths; the UI does no
filesystem reads or PCM decoding. Regular lanes use browser-style snapped
placement/overwrite; gaps and trailing empty Arrangement space use ordered new
track import. Main/Return rows are not media destinations. Stale painted geometry,
busy/closed/opening state and unsupported path groups refuse without enqueueing.

One audio file per drop is supported, matching the browser import worker's
single atomic import/Undo. Multi-file drops are refused as a whole before import;
no first-file selection or partial import occurs. Existing supported formats,
decode/resample settings, media validation and history semantics are retained.
Batch import is a separate enhancement. Native Finder
drop/cancellation and exact optimized macOS user confirmation remain unperformed.
Focused rendered placement/refusal and existing import/media/history checks are
recorded in PR172. Final full local CI and human review remain required; no
source-ready or native acceptance claim.
