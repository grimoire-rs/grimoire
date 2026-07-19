# Bootstrapping an Existing Skill Repo

You loaded this file because you have a repository of skills, rules, or
agents authored before grim existed — hand-rolled, agentskills.io-style,
or copied out of a `.claude/` project — and you're turning it into
something `grim publish` can release and announce.

Contents: [Inventory](#inventory) · [Fix Names](#fix-names) ·
[Backfill Catalog Metadata](#backfill-catalog-metadata) ·
[Generate publish.toml](#generate-publishtoml) · [Emit CI](#emit-ci) ·
[Local Verify Loop](#local-verify-loop) ·
[Announce Prerequisites](#announce-prerequisites)

## Inventory

Classify every artifact by the same shape rules `grim build` uses to
infer kind ([The Five Kinds][five-kinds]):

- A directory containing `SKILL.md` is a **skill**, whichever parent
  hosts it — the agentskills.io convention (`skills/<name>/SKILL.md`) and
  a Claude-specific project layout (`.claude/skills/<name>/SKILL.md`) are
  both directory-with-index shape, so grim infers identically either way.
- A bare top-level `.md` file is **rule**-shaped by default.
- Anything meant as an **agent** — a system prompt a client delegates
  to — needs `--kind agent` on every build/release call. A bare `.md` is
  otherwise indistinguishable from a rule, and grim never guesses from
  content; forgetting the flag is not an error, only a warning.

Run `grim build <path>` on each candidate as you go: its exit code and
kind are ground truth for what actually got classified, faster than
reasoning about it from tree shape alone.

## Fix Names

Rename anything whose directory (skill) or file stem (rule, agent)
violates the name charset before doing anything else — see the Universal
Invariants ([../SKILL.md#universal-invariants][universal-invariants]) for
the exact rule and the Exit-65 Triage
([release-checklist.md#exit-65-triage][exit-65-triage]) if `grim build`
already rejected one.

## Backfill Catalog Metadata

Catalog fields (`summary`, `keywords`, `repository`, `deprecated`,
`replaced-by`) live in a different place per kind — skills and agents
inside the frontmatter `metadata` map, rules at the top level of
frontmatter. Get this wrong and nothing errors; the field is just never
seen by `grim search`. See the Metadata-Location Asymmetry
([../SKILL.md#the-metadata-location-asymmetry][asymmetry]) for the full
picture, and the per-kind field tables in
[skill-spec.md](skill-spec.md#catalog-metadata),
[rule-spec.md](rule-spec.md#the-asymmetry), and
[agent-spec.md](agent-spec.md#frontmatter).

## Generate publish.toml

One manifest at the repo root drives the whole batch. A minimal skeleton,
mirroring the shape of grim's own catalog manifest:

```toml
registry = "ghcr.io"
version = "0.1.0"
version_prefix = "v"   # stripped from --version <git-tag>; "v" is the default, spelled out here

[skills.code-review]
repository = "acme/skills/code-review"

[rules.style]
repository = "acme/rules/style"

[agents.reviewer]
repository = "acme/agents/reviewer"
```

Write `repository` out verbatim on every entry (the `<ns>/<kind-subdir>/<name>`
shape) rather than leaning on `repository_prefix` — a renamed or moved
artifact then can't silently change its published address. `--version`
(the CI shape), the skip-existing default, and `push_registry` are all
covered in [Batch Publish][batch-publish] — read it before your first run.

## Emit CI

Don't hand-roll the workflow YAML. [Publishing from CI][ci] is the single
source of truth for both GitHub Actions and GitLab CI/CD, kept current as
grim's own CI integrations evolve — copy the recipe for your forge from
there. In short: publishing and announcing use two different credentials
— a registry token scoped to your own repo, and a separate one able to
push wherever the package index lives — and that page is where the exact
token, scope, and env var per forge are documented.

## Local Verify Loop

Iterate before your first release touches a registry:

```sh
grim build ./skills/code-review        # per artifact, until every kind is clean
grim build ./rules/style.md
grim build ./agents/reviewer.md --kind agent

grim publish --dry-run                 # whole-manifest validation, zero pushes
```

`grim build` catches per-artifact schema errors early; `grim publish
--dry-run` catches manifest-level mistakes — missing versions, bad
`repository` values, unknown keys — before anything touches the registry.

## Announce Prerequisites

`grim publish --announce` is what makes a release discoverable in `grim
search`, the TUI, and MCP browse. Two paths, both needing an API-capable
credential:

- **Auto-fork (default).** With a token, grim detects when you lack push
  access to the configured index, forks it for you, and opens the
  cross-repo pull/merge request automatically — no manual fork step.
- **Manual fallback.** Point `[announce] repository` (or `--announce-repo`
  for one run) at a fork you already maintain instead.

Either path needs a token — see [Publishing from CI][ci] for exactly
which credential, and where it comes from, per forge. Confirm current
flags with `grim publish --help`.

## Further Reading

- [Publishing from CI][ci] — the full workflow, both forges, credential model.
- [Batch Publish][batch-publish] — the full `publish.toml` schema.
- [Exit-65 Triage][exit-65-triage] — symptom → cause → fix table.

[five-kinds]: ../SKILL.md#the-five-kinds
[universal-invariants]: ../SKILL.md#universal-invariants
[asymmetry]: ../SKILL.md#the-metadata-location-asymmetry
[exit-65-triage]: release-checklist.md#exit-65-triage
[batch-publish]: release-checklist.md#batch-publish
[ci]: https://grimoire.rs/ci.html
