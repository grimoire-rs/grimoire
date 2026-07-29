# Host Your Own Index

An index is the phone book grim browses: it answers *"what packages
exist?"*, and every entry it holds is a pointer into an OCI registry.
Running one used to mean forking somebody else's repository and reading
their build script. It is now one command.

```console
$ npx @grimoire-rs/indexer init          # scaffold the repo
$ git push                               # your forge builds and serves it
$ grim config registry add acme --index https://acme.github.io/index
```

That is the whole loop. The scaffolder writes an index repository —
content tree, site config, contribution gate, CI for [GitHub
Pages][gh-pages] or [GitLab Pages][gl-pages] — and the forge you already
pay for hosts the result. There is no index server, no database, and no
account to create.

> **Which index?** The public one at [index.grimoire.rs][index-site] is
> the *default*, not the system. Your own index is a peer of it: same
> layout, same commands, same `grim search`. Consumers can configure
> several at once.

## Scaffold It {#scaffold}

The scaffolder ships on npm as [`@grimoire-rs/indexer`][npm]. It needs
[Node.js][node] 22.14 or newer and nothing else — `npx` fetches it per
run, so there is nothing to install globally:

```console
$ npx @grimoire-rs/indexer init acme-index
```

It asks six questions, each with a default:

| Prompt | What it decides |
|---|---|
| Index name | The identifier, and the default registry alias |
| Display title | The site's `<h1>`, header, and `<title>` |
| Base URL | Where the site will be served — `https://acme.github.io/index` |
| Registry alias | The alias consumers get in the copy-paste "add this index" block |
| Brand logo | Path or URL; blank renders the title as text |
| CI to scaffold | GitHub Actions, GitLab CI, or both |

Every answer is also a flag, and `--quick` skips the wizard entirely —
that is the form to use in a script:

```console
$ npx @grimoire-rs/indexer init acme-index \
    --quick --name acme --title "Acme Index" \
    --base-url https://acme.github.io/index --forge github
```

What lands on disk:

| Path | Purpose |
|---|---|
| `index/` | The content tree — one `metadata.json` per package |
| `index.config.json` | Site identity: title, base URL, branding, registry hint |
| `index-policy.json` | Committed allowlist the contribution gate reads |
| `.github/workflows/pages.yml` | Build and deploy to GitHub Pages |
| `.github/workflows/validate.yml` | The contribution gate on pull requests |
| `.gitlab-ci.yml` | Both of the above, as GitLab CI jobs |
| `README.md`, `.gitignore` | Contributor-facing docs and build-output ignores |

Re-running `init` is safe: a file that already matches is reported
`unchanged`, and one you have edited is left alone as `skipped` unless
you pass `--force`. That matters, because the CI files are the ones you
are most likely to touch.

Build it locally before you push anything:

```console
$ npx @grimoire-rs/indexer build
```

`dist/` then holds the rendered site, `all.json`, and a path-addressable
copy of every pointer — exactly what CI publishes.

## Push It {#deploy}

**GitHub.** Push to `main`, then set *Settings → Pages → Source* to
**GitHub Actions**. The `pages` workflow runs on every push to `main`
and deploys the built site. Nothing else is required — no deploy key, no
`gh-pages` branch, no personal access token.

**GitLab.** Push to the default branch. The `pages` job publishes
`public/`, and GitLab serves it at `https://<group>.gitlab.io/<project>`.
A private instance that refuses remote CI includes needs one edit — see
[Upgrading CI](#upgrading).

Either way the deployed site is two things at once: a searchable catalog
for humans, and `all.json` for grim.

## Use It {#consume}

Point grim at the URL and the index behaves like any other browse source:

```console
$ grim config registry add acme --index https://acme.github.io/index
$ grim search review
$ grim add ghcr.io/acme/skills/code-review
```

That writes a [`[[registries]]`](./configuration.md) entry, which is
per-project — so a repository can browse your internal index and the
public one side by side. Listings are cached under `$GRIM_HOME/catalog/`
with a one-hour TTL, `--refresh` forces a re-fetch, and `--offline`
serves the cache without touching the network. The rest of the transport
behavior is in [Consuming an Index](./package-index.md#consuming).

## Fill It {#packages}

An index entry is a pointer — a name, a kind, an OCI ref, and who owns
the namespace. There are two ways to add one.

**By hand**, for an index you curate yourself. Write
`index/<host>/<namespace>/<package>/metadata.json` and commit it:

```json
{
  "schema": 1,
  "name": "code-review",
  "kind": "skill",
  "ref": "ghcr.io/acme/skills/code-review",
  "description": "Review a diff against the team's checklist.",
  "owner": { "github": "acme", "id": 1234567 }
}
```

The field-by-field contract is the
[index specification](./package-index.md#spec-metadata). Note the ref
carries **no tag** — versions resolve live from the registry, so an index
can never serve a stale one.

**By announcement**, for an index other repositories publish into. Each
publisher points its `publish.toml` at your index and runs:

```console
$ grim publish --announce
```

grim pushes the packages to the registry, then opens a pull or merge
request against your index with the pointers — forking automatically when
the publisher has no push access. The full behavior is in
[Announcing Packages](./package-index.md#announcing).

> **One repo for both?** `init --with-skills` scaffolds the combined
> layout: your skills under `skills/`, the index that lists them beside
> it, and a `publish.toml` that announces into itself. Convenient when
> one team owns both sides; the separate-repos shape is the default
> because an index is a distribution database that many publishers
> announce into.

## Accept Contributions {#gate}

An index that takes pull requests needs to answer one question
mechanically: *is the author allowed to write this entry?* That is what
the scaffolded `validate` job does, on both forges. Exit 0 means the
contribution is eligible for auto-merge; anything else means a human
looks at it.

The gate refuses a contribution that:

- touches any path other than `index/<host>/<ns>/<pkg>/metadata.json`,
- claims a namespace whose forge owner is not the author (checked against
  the **numeric** account id, so a recycled login cannot inherit it),
- fails the metadata schema,
- points at a registry host outside `index-policy.json`, or
- names an OCI ref the registry cannot serve — publish before you
  announce.

It judges all of that against the **committed** state of your repository,
never the state the contribution proposes, so a pull request cannot
allowlist its own registry host in the same change.

`index-policy.json` is the knob worth knowing:

```json
{
  "version": 1,
  "registryHosts": ["ghcr.io"],
  "reservedNamespaces": ["grim", "grimoire", "index"],
  "trustedBots": []
}
```

Widening trust is a reviewed commit rather than a CI variable, which is
the point.

> **Requiring the check on GitHub**: the context to require in branch
> protection is `validate / validate` — the caller job name, then the
> called one. Requiring plain `validate` names a context that never
> reports, which blocks every pull request forever and looks exactly like
> the gate rejecting the contribution.

Two limits, stated plainly. On GitHub, a pull request opened with the
built-in `GITHUB_TOKEN` triggers no workflows — so in the combined
(`--with-skills`) layout, your own CI's announce arrives **ungated** and
wants a human. On GitLab, a merge request supplies its own
`.gitlab-ci.yml` and can therefore delete the gate job; enforce it
instance-side with a required merge check, or treat the GitLab gate as
advisory.

## Make It Yours {#branding}

`index.config.json` is the whole customization surface, and every key is
optional:

| Key | Effect |
|---|---|
| `brand`, `brandMark` | Header text and its accent-styled monospace prefix |
| `description`, `tagline` | Meta description and the hero paragraph |
| `logo`, `favicon` | Header logo (and default link-preview image), tab icon |
| `install` | The hero's installer one-liners, per platform. `[]` hides the strip |
| `registry` | The copy-paste "add this index" block. `null` omits it |
| `repoUrl`, `docsUrl` | Header links. `null` hides either |
| `vscodeExtension` | The VS Code deep link on each package. `null` drops it |
| `footerNote`, `attribution` | Footer sentence, and the "built with Grimoire" line |
| `customCss` | A CSS file inlined after the default tokens — it wins over them |

Colors, spacing, and the kind badges are CSS custom properties defined
for light **and** dark, so `customCss` overriding three tokens restyles
both without forking the renderer. There is deliberately no
component-override API yet: publishing one would freeze a prop contract
per slot, and that is not a promise worth making this early.

## Keep It Fresh {#enrich}

An index stores pointers, so a site built from pointers alone lists names
and says *No README available* on every page. `enrich` fixes that by
reading the registry:

```console
$ npx @grimoire-rs/indexer enrich    # needs `grim` on PATH
```

It writes README, changelog, logo, contents, and the resolved version
list into `enrich/`, skipping any package whose digest has not moved
since the last run. The scaffolded CI installs grim and runs this before
every build, so the deployed site stays current without a scheduled job.
It is also the only step that goes online — set `enrich: false` (GitHub)
or `GRIM_INDEXER_ENRICH: "false"` (GitLab) for a pointers-only site that
never leaves the runner.

## Upgrading CI {#upgrading}

The scaffolded workflows are deliberately thin: they call reusable
workflows that the indexer repository owns, pinned by tag.

```yaml
jobs:
  pages:
    uses: grimoire-rs/indexer/.github/workflows/index-pages.yml@v0.2.2
    with:
      grim-indexer-version: "0.2.2"
```

So picking up a CI fix is bumping a ref — never re-scaffolding over your
edits. [Renovate][renovate]'s built-in `github-actions` manager bumps it
for you. GitLab works the same way through a pinned `include: remote:`;
an instance that blocks remote includes vendors the file into the repo
and switches to `include: local:`.

## Private and Internal {#private}

Nothing above assumes a public repository. Three shapes work:

- **Private repo, git transport.** Skip Pages entirely and hand consumers
  the clone URL: `index = "https://gitlab.example.com/platform/index.git"`.
  grim shallow-clones and walks the tree, authenticating through ambient
  git credentials — a credential helper or ssh agent. It never prompts.
- **Private repo, private Pages.** GitLab Pages can require a session on
  the project; the index then resolves for anyone who can read the repo.
- **Air-gapped.** `index-policy.json` bounds which registries entries may
  point at, so an internal index can refuse anything not on your mirror.

The corporate GitLab walkthrough — release mirrors for the grim binary
itself, auto-merge by group membership, publishers with zero forge
configuration — is [Self-Hosted GitLab Setup](./self-hosted-gitlab.md).

## Without the Toolchain {#plain-git}

The npm package is a convenience, not a dependency. A git repository
containing `index/<host>/<ns>/<pkg>/metadata.json` **is already a working
index** — grim's git transport walks the tree directly:

```toml
[[registries]]
alias = "team"
index = "https://gitlab.example.com/platform/index.git"
```

No build step, no CI, no site. Reach for the scaffolder when you want the
browsable catalog, the contribution gate, or both.

<!-- external -->
[gh-pages]: https://pages.github.com/
[gl-pages]: https://docs.gitlab.com/ee/user/project/pages/
[node]: https://nodejs.org/
[npm]: https://www.npmjs.com/package/@grimoire-rs/indexer
[renovate]: https://docs.renovatebot.com/modules/manager/github-actions/

<!-- grimoire -->
[index-site]: https://index.grimoire.rs
