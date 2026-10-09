# MUSICAL-TIME-001 — musical map workflow requirements

Status: public documentation reviewed; implementation in progress; native Live
numerical behavior unverified.

- Source: [official Live 12 Arrangement View manual, §§6.1 and 6.5](https://www.ableton.com/en/live-manual/12/arrangement-view/), read 8 October 2026.
- Evidence type: public product workflow documentation, not binary analysis or
  an experiment on the installed executable. No proprietary fixture was used.
- Documented requirements: the ruler presents musical positions; signature
  controls affect the current location; meter markers may occur inside a measure,
  creating a partial bar. Marker numerators may have one or two digits and the
  supported denominator list is 1, 2, 4, 8, 16.
- Architecture implication: a single global signature or strict bar-boundary
  validation would exclude a documented workflow. Use ordered meter markers with
  fragmentary bars. The initial proposed bar-boundary restriction was corrected
  before accepting the implementation contract.
- Independent Hex design: fixed-point beats, bounded tempo/meter maps,
  nearest-frame conversion, right-continuous boundaries, ceiling-based bar
  prefixes, opt-in snapping and preservation of frame-anchored legacy audio.
  `docs/musical-time.md` owns these original policies.
- Unknown: exact Live fractional-beat/frame rounding, bar labeling at partial
  boundaries, meter edits while playing, tempo curve interpolation, warped audio
  anchoring and synchronization of Session/Arrangement changes. Public workflow
  prose does not establish these algorithms.
- Planned verification: original map/PCM fixtures through edit/history, strict
  migrations, compact native ruler and snapped drag. A controlled Live comparison
  is still required before numerical parity can be claimed.
