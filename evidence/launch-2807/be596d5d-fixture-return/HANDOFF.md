# Canonical413 fixture-only return

Source be596d5dbbdc05a3ce8aedbc0f05d90e4920a647, tree 338a8e6c671de3f15af348b28d898ab908ef0457, direct child of a1a4391e9f5e502a22716423c5bd122a5c9f2290. Normal FF push, no main/prospective history rewrite. Only scripts/tests/test_launch_restart_compile.py changed (two methods, +44/-4).

CAUSE AND LAW
The old positive .2s assertion required the fake Python interpreter to launch faster than .2s; the negative assumed its invocation was written before that deadline. A controlled OWNED child adapter delays exec by .5s, reproducing EXACT old1ERROR+1FAIL. New same-adapter2PASS; all owned children reaped. The initial harness had two path-setup errors before being corrected; raw log retained separately, not the red proof.

Positive test now captures compile/runtime wait arguments [30,5,5] under adequate fixture budgets, retains compile delay .35/one compile/two operations. Negative test waits boundedly for its own helper's full invocation record, then actual .2s wait expires on native_sleep60 and production owned-child cleanup reaps it; asserts no recompilation/retry/workspace. This deliberately avoids using interpreter startup as a product performance oracle. Production15s still includes startup, no readiness hook added.

CHECKS
Actual Linux Python3.12.14:22+20+25=67PASS/0skips/all3exit0. Two affected cases ALSO passed controlled .5s slow-start oracle; these2 are not added to unique67. Five guards pass; Python3.9 grammar/current Python compile and diff checks pass. Actual Python3.9 runtime/Swift NOT run here. First guard failed missing cargo PATH after suites had already passed; configured toolchain activated and only remaining guards run. Logs preserve failure, no suite repeated for environment correction. checks.json uses actual unittest-reported seconds, not reread elapsed times.

Production Python, Swift, launch, local-ci.py, hex-test-build.py, docs and other two suites byte-identical to a1a4 (unchanged-files.json). No app/native/Swift/signals/userdata/reset/hostedCI/fullCI/merge. Source4 no Sampler395 or Beats449 edits.

HISTORY / LIMITS
Root reports Mac3.9.6 a1a4 first22=20PASS1ERROR1FAIL/8.741s/exit1; later20+25+actual Swift smoke NOT RUN. Actual production Swift15 not exercised. Report SHA977cb6e8b59ec67c8704ec5e06d70f8c2e6e92a93366bb71762311390b8ad798, manifestedd57e53568f767e103bbcb4acceae56063ef3dafef75f0694b7a102d39fa6f5 (77 verified), raw15af60ae3ab25f255f07055ceec05191c6075b7bccc66e918b831659fa005701. These are root supplied references, not cloud-recovered raw Mac bytes. Original cloud a1a4 archive4897c923f8d7a5d0c17c472c2e43165f6cb81dc399dfff4711209980a781e4ae remains immutable at evidence ref1e8d195b/evidence/launch-2807/a1a4-compiled. Preserve original interpreted timeout, diagnostic compiled result and later merger prospective history independently.

NEXT
Root serial Mac ONLY: exact67 then production compiled absent snapshot/quit/force using pinned supplied smoke. AppKit running fixture1–4 still UNEXECUTED/separate review. No running Hex/real process control, actual installation/delivery or full native qualification. Sole merger owns review/prospective full localCI/integration/merge; Pulse InProgress until actual merge. New native result gets THIS exact source identity; do not relabel old failures.
