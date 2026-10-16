# Native time-selection profiling

Issue #14 retains its canonical merged PR #31. This supplementary tooling measures
native event dispatch to selection paint, following-frame observation, and worker
completion separately for identical hybrid content on one versus 100 tracks.
It does not change editing behavior or establish physical display/audio deadlines.

The disposable profiling window uses offline audio, a normal session worker, and
settings under the supplied fixture directory. It never opens a user project or
writes user preferences/recovery data. Fixture generation and loading are untimed.

Planned commands:

```sh
cargo run --release -p hex-session --example time_selection_fixture -- /absolute/fixture-dir
cargo build --release -p hex-desktop --locked
target/release/hex-desktop --profile-time-selection /absolute/fixture-dir > raw.csv
```

Each case uses five warmups and 40 measured samples. Both project sizes have 50
audio plus 50 MIDI clips per track, eight notes per MIDI clip and one shared WAV.
The native harness must prove project-wide selection, queued non-noop operations,
expected worker status/revision, and a painted result; errors/timeouts are failures.
Raw evidence, exact source/build identity and analysis are recorded separately.
