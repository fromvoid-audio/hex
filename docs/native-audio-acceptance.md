# native audio acceptance — issue #23

## owner-reported results, 16 October 2026

Tested app: hex 0.1.0, source
`f82e6271445ed4bba0ced7590528ab952f467614`, installed at
`/Users/portalsurfer/dev/hex/artifacts/hex.app`. The owner confirmed the
version/source in the app and used built-in audio. This is the optimized build
from [PR205](https://github.com/PORTALSURFER/hex/pull/205), merged as
`0ff955a8f47f705cae31409db57119aa03264b2f`; both commits have the same Git tree.
The preflight verified every receipt-listed file hash and the executable's
ad-hoc signature. Its bundled full local-CI report records 16 passing steps,
`stale=false`, on the exact candidate. No new application build is claimed here.

The owner performed the following guided native checks and reported:

- Ten Play/Stop cycles: initial attacks present without duplication, continuous
  playback, no clicks/dropouts or lingering output, and Stop returned the cursor
  to the start.
- Five Stop-during-editing trials: prompt Stop each time, with no clicks,
  dropouts, lingering output or unexpected restart.

These are subjective native observations reported by the owner in the issue's
working chat. Gain/edit pressure does not establish actual command-queue
saturation. Machine model, OS version, sample rate, buffer size and fixture
identity were not recorded; do not infer them from built-in audio.

The owner then requested merge, saying “it all looks good to me”. This records
approval of the above tested build and reported cases, not a pass for unreported
checks. [PR245](https://github.com/PORTALSURFER/hex/pull/245) preserves this evidence
and the intake plan; it introduces no application behavior change.

## outstanding acceptance

- Audio restart/recovery and project/cursor preservation: instructions supplied,
  but no result reported.
- Device selection through the macOS default output and physical disconnect:
  not tested; external-interface disconnect requires suitable hardware.
- Sustained native stress and diagnostic counter observations: not performed.
- Physical timing and captured first-transient offsets: not measured; require a
  documented capture route, fixture, clock and tolerance. Audible attacks do not
  establish sample timing or physical deadlines.
- Core Vector/Operator, Utility, Saturator and EQ8 lifecycle/listening cases:
  no specific results reported in this session.

Pulse #23 remains the acceptance ledger. The 16 October roadmap limits 0.1 to
core reliability and the four core devices; expanded device listening, export,
100-track/polyphony and streaming performance remain 0.2 work under #2108,
#2226 and #24. Preserve prior evidence and pending gates without making those
broader goals prerequisites for this acceptance record.
