# Finder audio import — pulse2385 / pr172

The local OS-drop adapter connects GPUI ExternalPaths to the existing canonical
bounded audio import worker. The platform retains file paths; the UI does no
filesystem reads or PCM decoding. Regular lanes use browser-style snapped
placement/overwrite; gaps and trailing empty Arrangement space use ordered new
track import. Main/Return rows are not media destinations. Stale painted geometry,
busy/closed/opening state and unsupported path groups refuse without enqueueing.

This is a partial implementation: the existing worker accepts one path per
atomic import/Undo. The adapter currently refuses multi-file drops explicitly.
The requirement for atomic multi-file drop is awaiting coordinator clarification;
multiple sequential requests must not masquerade as one Undo. Native Finder
drop/cancellation and exact optimized macOS user confirmation remain unperformed.
Focused rendered placement/refusal and existing import/media/history checks are
recorded in PR172. Final full local CI and human review remain required; no
source-ready or native acceptance claim.
