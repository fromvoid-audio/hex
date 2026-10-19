# SAME455 / Pulse2853 denormal owner return — Root → sole merger

Frozen final9afb8f2e916e775aa6d288502c5cbc23d4f6b386/treeb88ba5c1d4026bab0d61a40692a4968d45cb4064, exact baseline99e7f5eb0cddd8e298cc8533888d48341a88d3f2/tree217c4a03835edbdd18a48d6e68296f168420247d. Published isolated source-fix/455-denormal-99e7; preserve canonical99e7/history and compose ONLY three declared files. No new PR, full21, self-merge or native/app action. Original full21 at99e7 remains historical review-blocked evidence.

Executed RED: actual prepared provider, 96 configurations: six rates8000/22050/44100/48000/96000/192000 ×DC-step/impulse ×Peak/RMS/bypass/both mode transitions/fully-dry. Actual8s silence then257more frames retained nonzero subnormal energies in all96:8000plateau2e-322,192000plateau4.743e-321. Real output stayed exact digitalzero. Baseline test-only21beff11789d4fbad620808959d0f332f7dd293a/source unchanged provider99e7; raw1FAIL/exit101 retained. This proves state persistence, not native CPU spikes, audio glitches or a measured exact7.3s threshold.

Production delta SIX added lines only: private power floor1e-30 (-300dBFS detector amplitude), local post-recurrence energy reset below floor. This is180dB below existing -120dB reference and far below threshold/knee. Exact existing update arithmetic above floor/10ms coefficient/stereo max-after-averaging/Peak fastpath/5ms transition/DryWet/hosttrims/bypass/copy/reset/codec3 remain. Subfloor increments are discarded; this is a numerical floor, not a new noise gate or control. No FTZ/floating environment/globalDSP/API/state/schema/ownership changes. Fixed private history obeys zero-or-normal lower bound; finitef32 input squared/coefficient cannot introduce f64 subnormals into that invariant. No broad claim about every other legacy detector/follower state.

Evidence scopes:
- edbef1d829084181905c5f14a0abe0e37f88b676/treef6081b383b239b333ebc3e9eaf387e3d3e5a3aca CLEAN12/12PASS147.224s.27provider tests (old24+three new),96DC/impulse cases,12independent analytic exponential crossing cases, six irregular partition/scratch/cancel/retarget/invalid/tinyinput/reset cases.9actualGPUICompressor Track+Return controls/A-B/project+preset persistence PASS. RealTrack+Return callback allocation test PASS, calibrated private36rate/mode/mix permutations including measured floor crossing: zero TOTAL/net allocations and frees. Provider+UI strict/architecture/style/provenance/dependency/fmt/diff pass.
- Final9afb changes ONLY test stimulus extension and docs: preserves DC+impulse cases, adds48periodic-stereo-tone cases,144total. CLEAN8/8PASS on exact9afb:27provider tests/all144cases, strictprovider and sixguards. Production compressor.rs and EVERY UI/Audio/Engine/Session/Storage/binding source byte-identical to edbef. Do NOT relabel edbef12stage receipt as9afb execution; both exact receipts/source-proof included. No unnecessary unchangedUI/callback repeat for test/doc-only extension.
- Original Peak/DryWet PCM goldens, independent DC/impulse/burst/sine/quadrature/phase/rate/validation/lifecycle/stereo/codec cases unchanged. Original test bytes and current99e7A/B/mode/currentformat tests untouched. Actual newsource UI9 includes shared447 Switch/both Copy directions, selectedbank preview/caption, distinct ID8/ID7 banks, project/preset reopen and globaltrims. No provider-specific bank fork.

Allocation harness reuses existing original36case probe, with unique private Cargo identity and measured long silence through actual public runtime methods, calibration detects4096-byte allocate/free churn. Runtime factories/preparation and stack buffers established outside measured processing, no mocked callback. Existing whole Track/Return callback test runs separately. No allocation-counter dependency added to production workspace.

All raw original failures/green/final receipts and source histories preserved; earlier455b8f3/127129/99e7and453/454/310/459 packets remain frozen. Merger owns exact prospective combined-tree required21/fresh independent review/merge. Pulse remains InProgress until actually merged. Root alone owns optimized native/listening/device acceptance. No new Mac job is necessary to establish these private mathematical/allocator results; optional Root-only physical followup may play transient→long silence in Peak/RMS/bypass and verify ordinary mode/mix/A-B behavior on actual eventualdelivery, without claiming audible/numerical Live parity or W12 completion.

Next authorized queue:2861stuckpan +2867capturedrelease critical,2860feedback/2862MIDIneighbors serially; then queued459newinfrarepros andEQ2859/search2863/view2864–2866 byRoot priority. No new worker.

## Source-bound exact receipts

{
  "final": {
    "source": "edbef1d829084181905c5f14a0abe0e37f88b676",
    "seconds": 147.224,
    "summary_sha256": "cb5c6febee8622c0ab919ef582f4a68621062c66ea703c35616e629ffd4482bb",
    "content_sha256": "27acf7f6b204e84a13e5b12729b7584fe23acf893425298ed0bec4788de0ec01"
  },
  "final-tone": {
    "source": "9afb8f2e916e775aa6d288502c5cbc23d4f6b386",
    "seconds": 28.753,
    "summary_sha256": "76be25f3d6092971fbce32c10215b4fcab8d7c5dd3f2428b9184f9c2b1bddf6e",
    "content_sha256": "42914168b3ddc3bdc4ed64f1e74faef1e0bee6b4c374e9cdf6c488fccf98aab5"
  }
}
