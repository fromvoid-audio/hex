# cloud setup

Use the existing private hex environment bound to `PORTALSURFER/hex`. Preserve its
sharing, repository, network/proxy and credential settings. Inspect actual current
configuration and readiness; a repaired running container is not a published
setup or a verified fresh instance.

Where provided, activate the configured tools before checks:

```sh
. /workspace/.hex-tools/activate.sh
rustc --version
cargo --version
pkg-config --modversion alsa fontconfig freetype2 xcb xcb-xkb xkbcommon xkbcommon-x11
```

Verify the toolchain against the task's pinned configuration. Large engine
fixtures may require `RUST_MIN_STACK=16777216`. Missing native headers or libraries
need an environment fix; do not work around a source error by changing dependencies.

Before network operations inspect supported readiness and non-secret
`/etc/codex/network-policy.json`. Use only permissions supported and authorized by
the current executor. Do not unset proxies, disable TLS, broaden allowlists, copy
credentials or modify protected runtime state. Report the exact sanitized refusal.

Test a setup draft before publication, then validate a newly created instance's
actual configuration and files. Never claim an existing instance proves fresh
startup. Store environment faults and results in Pulse, with source, command,
configuration and platform limits; keep secrets out of logs.

Run [local checks](local-ci.md) on exact task source. Linux compilation and
headless tests do not establish macOS GPUI, CoreAudio, physical deadlines or
listening acceptance. No hosted CI or paid infrastructure is authorized here.
