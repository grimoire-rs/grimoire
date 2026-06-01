# Publishing Skills and Rules

Consuming artifacts is only half of Grimoire. The other half is producing them:
turning a local skill directory or rule file into a versioned OCI artifact that
others can [`grim add`](./commands.md#add).

## Author locally

A **skill** is a directory containing a `SKILL.md` and any supporting files; a
**rule** is a single Markdown file. Grimoire detects which one you mean from the
path — a directory packs as a skill, a `.md` file packs as a rule — and
`--kind` overrides the guess when you need to.

## Validate before you push

[`grim build`](./commands.md#build) validates and packs an artifact **without**
pushing it. Run it while iterating to catch a malformed skill before anyone
else sees it:

```sh
grim build ./code-review
grim build ./rust-style.md --kind rule
```

## Release

[`grim release`](./commands.md#release) validates, packs, and pushes to a
registry in one step. Give it the source path and the release reference:

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3
```

### Cascade tags

A release does more than push one tag. From a `1.2.3` version it also moves the
**floating** tags that consumers track — `1`, `1.2`, and `latest` — to the new
digest. That is what lets a consumer who declared `:1` pick up `1.2.3` with a
plain [`grim update`](./commands.md#update).

### Dry runs and overwrites

Preview the exact push plan — every tag and the digest each will point at —
without touching the registry:

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3 --dry-run
```

An exact-version tag is immutable by default: if `1.2.3` already exists and
points at different bytes, the release refuses rather than rewrite history.
Pass `--force` only when you deliberately mean to move it.

## Authenticate

Grimoire pushes over standard OCI, so it reuses your existing registry
credentials — the same login your container tooling uses. Authenticate once
with your registry (for example, `docker login ghcr.io`) and `grim release`
inherits it.

<!-- external -->
[ghcr]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
