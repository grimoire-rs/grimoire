# grim-mirror

Republishes skills, rules, agents and bundles from git repositories and
Claude plugin marketplaces into an OCI registry, using `grim publish`.

Status: Unstable until it has its own repository. Source only, no releases.

## Install

```sh
cargo install --locked --git https://github.com/grimoire-rs/grimoire --rev <sha> grim-mirror
```

## Use

```sh
grim mirror check              # dry run, publishes nothing
grim mirror sync               # publishes
```

`grim` dispatches `grim mirror` to the `grim-mirror` binary on `PATH`.
See [`examples/mirror.toml`](examples/mirror.toml) for the file format and
the docs page `mirroring` for the full contract.
