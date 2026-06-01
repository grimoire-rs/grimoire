# Installation

`grim` is a single self-contained binary. Once it is on your `PATH` there is
nothing else to configure.

## Pre-built binaries

Every release publishes archives for macOS, Linux, and Windows on both
`aarch64` and `x86_64`, each accompanied by a SHA-256 checksum and a
[CycloneDX][cyclonedx] software bill of materials. Download the latest from the
[releases page][releases].

| Platform | Asset |
|----------|-------|
| macOS (Apple Silicon) | `grimoire-aarch64-apple-darwin.tar.xz` |
| macOS (Intel) | `grimoire-x86_64-apple-darwin.tar.xz` |
| Linux (ARM64) | `grimoire-aarch64-unknown-linux-gnu.tar.xz` |
| Linux (x86-64) | `grimoire-x86_64-unknown-linux-gnu.tar.xz` |
| Windows (ARM64) | `grimoire-aarch64-pc-windows-msvc.zip` |
| Windows (x86-64) | `grimoire-x86_64-pc-windows-msvc.zip` |

On Linux or macOS, download the right archive, extract it, and move the `grim`
binary onto your `PATH`. Each archive also carries the license, README, and
changelog alongside the binary:

```sh
curl -LO https://github.com/michael-herwig/grimoire/releases/latest/download/grimoire-x86_64-unknown-linux-gnu.tar.xz
tar -xf grimoire-x86_64-unknown-linux-gnu.tar.xz
install -m 0755 grim ~/.local/bin/grim
```

On Windows, unzip the archive and place `grim.exe` somewhere on `PATH`.

## Build from source

With a [Rust toolchain][rustup] installed (Grimoire targets the stable 2024
edition), install straight from the repository:

```sh
cargo install --git https://github.com/michael-herwig/grimoire grimoire
```

Or clone and build a release binary at `target/release/grim`:

```sh
git clone https://github.com/michael-herwig/grimoire.git
cd grimoire
cargo build --release
```

## Verify

```sh
grim --version
```

If the command prints a version string, you are ready for the
[Quick Start][quickstart].

<!-- external -->
[cyclonedx]: https://cyclonedx.org
[releases]: https://github.com/michael-herwig/grimoire/releases
[rustup]: https://rustup.rs

<!-- internal -->
[quickstart]: ./quickstart.md
