# hex

hex is a native Rust digital audio workstation using GPUI and CPAL, currently in active alpha for macOS arm64.

- Arrange audio and MIDI tracks alongside a clip/scene Session view.
- Mix with built-in native devices and hosted VST3 plug-ins, with MTS-ESP tuning support.
- Render projects from the CLI or launch the desktop app directly from source.

## Build

Requires macOS arm64 (Xcode Command Line Tools) and the Rust toolchain pinned in `rust-toolchain.toml` (`rustup`).

```sh
cargo run --release --locked            # build and run hex
cargo run --release --locked -- --demo  # launch with the demo session
cargo build --release --locked          # build release binary only
./launch                                # pull latest, prepare VST3 helpers, and run
```

Run `python3 scripts/local-ci.py` to execute local checks.

## About

I’m [PORTALSURFER](https://soundcloud.com/portalsurfer). I make music and build instruments and tools for making it under [fromvoid](https://github.com/fromvoid-audio). I use AI extensively in that work and share these projects so more people can make music, whatever their budget.

I build this software for myself first and foremost and offer no guarantees of support. That said, I welcome code, documentation, and interesting ideas, and I’m happy to add them when we align on the direction.

Copyright wasn't just inconvenienced by AI; perhaps it was always the wrong bet.

We treated ideas like property, then acted surprised when progress slowed and wealth pooled upward. But evolution has always followed a simple pattern: copying, mutation, and selection. Ideas, like living things, develop through variation and iteration.

Perhaps the mistake was never copying, but our insistence on preventing it.

Soon, software will be something we simply wish into existence, endlessly adapting to our needs. When creation becomes abundant, artificial scarcity loses its purpose.

fromvoid is built in the open, not as charity, but as a refusal to restrict the evolution of ideas.

Copy. Mutate. Select. Repeat.

## Support

If you’d like to help cover development costs, you can chip in on Ko-fi:

<a href='https://ko-fi.com/F7X2225YCF' target='_blank'><img height='36' style='border:0px;height:36px;' src='https://storage.ko-fi.com/cdn/kofi6.png?v=6' border='0' alt='Buy Me a Coffee at ko-fi.com' /></a>

---

[Development workflow](docs/development-workflow.md) · [Local CI](docs/local-ci.md) · [AGENTS.md](AGENTS.md) · [Attribution](ATTRIBUTION.md)
