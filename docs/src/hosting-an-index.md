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

> **Want it as a checklist?** [Run your own index][wizard] walks the same
> setup step by step, switched between GitHub and GitLab. This page is the
> reference behind it. On GitHub you can also skip step one entirely and
> start from the [`index-template`][template] repository.

> **Which index?** The public one at [index.grimoire.rs][index-site] is
> the *default*, not the system. Your own index is a peer of it: same
> layout, same commands, same `grim search`. Consumers can configure
> several at once.

> **If you have run a Helm chart repository, you already know this
> shape.** A [chart repository][helm-repo] is a static `index.yaml` plus
> files on any webserver — no repository server to operate, which is why
> hosting one on Pages became the norm. A grim index is the same trade:
> pointers compiled to static JSON, a site built beside it, and the forge
> doing the serving.

## Scaffold It {#scaffold}

The scaffolder ships on npm as [`@grimoire-rs/indexer`][npm]. It needs
[Node.js][node] 22.14 or newer and nothing else — `npx` fetches it per
run, so there is nothing to install globally:

```console
$ npx @grimoire-rs/indexer init acme-index
```

It asks a short series of questions, each with a default. Run it inside a
clone and it reads the `origin` remote first, which answers the two that
are otherwise awkward — which forge, and where Pages will serve you:

| Prompt | What it decides |
|---|---|
| Index name | The identifier, and the default registry alias |
| Display title | The site's `<h1>`, header, and `<title>` |
| Repository URL | Read off `origin` when there is one — otherwise asked |
| Base URL | Where the site will be served. Derived from the repository; type a custom domain to override |
| Registry alias | The alias consumers get in the copy-paste "add this index" block |
| Brand logo | Path or URL; blank renders the title as text |
| Registry host | The committed allowlist the contribution gate bounds every entry by |
| Initialize git? | Offered when the directory is not already a repository |
| Install now? | Writes `package-lock.json` — CI installs from it, so answer yes |

The forge is derived from the repository host rather than asked. One
repository runs one pipeline; rendering both left every index carrying CI
it would never run.

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
| `index.config.json` | Site identity: title, base URL, branding, and the `ci` block |
| `index-policy.json` | Committed allowlist the contribution gate reads |
| `package.json`, `package-lock.json` | The scripts, and the pin on which renderer builds and judges this index |
| `.github/workflows/pages.yml` | Build and deploy to GitHub Pages |
| `.github/workflows/validate.yml` | The contribution gate on pull requests |
| `.github/workflows/verify-ci.yml` | Re-renders the workflows and fails on drift |
| `.gitlab-ci.yml` | All of the above, as GitLab CI jobs |
| `README.md`, `.gitignore` | Contributor-facing docs and build-output ignores |

Only one forge's CI is written — whichever the repository host implies.

Re-running `init` is safe, and every prompt defaults to what this index
already answered — so pressing Enter through the questions you do not
care about changes nothing. A file that already matches is reported
`unchanged`, one you have edited is left alone as `skipped` unless you
pass `--force`, and two are reported `preserved` and never rewritten at
all: `publish.toml` and `index-policy.json` hold the packages you declare
and the trust settings the gate reads, and no scaffold can regenerate
either. Edit those two directly.

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
[Upgrading CI][upgrading].

Either way the deployed site is two things at once: a searchable catalog
for humans, and `all.json` for grim.

> **Proven where?** The GitHub loop — scaffold, push, Pages, announce,
> gate, auto-merge, consume from a clean `GRIM_HOME` — was rehearsed end
> to end against live repositories. The GitLab jobs run the same tool
> with the same arguments and are covered by tests, but that leg has not
> been exercised against a live instance yet.

## Use It {#consume}

Point grim at the URL and the index behaves like any other browse source:

```console
$ grim config registry add acme --index https://acme.github.io/index
$ grim search review
$ grim add ghcr.io/acme/skills/code-review
```

That writes a [`[[registries]]`][config] entry, which is
per-project — so a repository can browse your internal index and the
public one side by side. Listings are cached under `$GRIM_HOME/catalog/`
with a one-hour TTL, `--refresh` forces a re-fetch, and `--offline`
serves the cache without touching the network. The rest of the transport
behavior is in [Consuming an Index][consuming].

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
[index specification][spec-metadata]. Note the ref
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
[Announcing Packages][announcing].

### One repo for both {#packages-combined}

`init --with-skills` scaffolds the combined layout: your skills under
`skills/`, the index that lists them beside it, a `publish.toml` that
announces into itself, and a **release pipeline** to run it. Convenient when
one team owns both sides; the separate-repos shape stays the default,
because an index is a distribution database that many publishers announce
into.

The pipeline needs one decision, which `init` asks for and `ci.publish`
records — when a release happens:

| `ci.publish` | Fires on |
|---|---|
| `tag` | A `v*` tag. Cutting a release is a deliberate act |
| `default-branch` | Every push to the trunk. A version bump in `publish.toml` *is* the release |
| `never` | Nothing is rendered. The default, and the only right answer for an index that owns no packages |

Either way the job runs `grim publish --announce`, and
[skip-existing][skip-existing] makes a re-run over unchanged versions a
no-op — which is what makes the every-push trigger reasonable rather than
reckless. Registry credentials default to the zero-setup path on each forge
(the built-in token against GHCR, the job token against GitLab's registry);
publishing elsewhere is repository variables, not an edit to the generated
file.

> **Two things this layout costs you.** On GitHub the announce needs
> *Settings → Actions → General → "Allow GitHub Actions to create and
> approve pull requests"*, which is off by default — without it the packages
> publish and the pull request then fails to open. And that pull request is
> **not gated**: GitHub runs no workflows on one opened with the built-in
> token, so `validate` never fires on it. Review it by hand.

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
> protection is `validate` — the job key in the committed workflow. (It
> was `validate / validate` while the scaffold emitted thin callers of
> reusable workflows; those are gone.) Add the check and the branch rule
> together: requiring a context that never reports blocks every pull
> request forever and looks exactly like the gate rejecting the
> contribution.

Passing contributions can land unattended. Set `"autoMerge": true` in the
`ci` block and re-render with `npm run ci`; the generated job squash-merges
the pull request the gate passed, pinned to the commit the gate actually
judged, then redeploys. It is **GitHub-only** and deliberately so — see the
GitLab limit below.

Two limits, stated plainly. On GitHub, a pull request opened with the
built-in `GITHUB_TOKEN` triggers no workflows — so in the combined
(`--with-skills`) layout, your own CI's announce arrives **ungated** and
wants a human. On GitLab, a merge request supplies its own
`.gitlab-ci.yml` and can therefore delete the gate job; enforce it
instance-side with a required merge check, or treat the GitLab gate as
advisory.

## Make It Yours {#branding}

`index.config.json` is the whole customization surface, and every key is
optional — a missing file renders the defaults. The ones worth knowing:

| Key | Effect |
|---|---|
| `site` | The canonical deploy URL. Drives absolute link-preview URLs; `init` writes the base URL you gave it |
| `brand`, `brandMark` | Header text and its accent-styled monospace prefix |
| `description`, `tagline` | Meta description and the hero paragraph |
| `logo`, `favicon` | Header logo (and default link-preview image), tab icon |
| `install` | The hero's installer one-liners, per platform. `[]` hides the strip |
| `registry` | The copy-paste "add this index" block. `null` omits it |
| `repoUrl`, `docsUrl` | Header links. `null` hides either |
| `vscodeExtension` | The VS Code deep link on each package. `null` drops it |
| `footerNote`, `attribution` | Footer sentence, and the "built with Grimoire" line |
| `footerLinks` | Extra footer links — `[{ "label": "privacy", "href": "https://…" }]`. Absolute URLs only, none by default. Where the pages behind them are required at all depends on where you are and what the index is for |
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
It is also the only step that goes online — set `"enrich": false` in the
[`ci` block](#upgrading) and re-render for a pointers-only site that never
leaves the runner.

## Changing the CI {#upgrading}

Your index owns its pipeline. The workflow files are **committed in your
repository** and rendered from the `ci` block of `index.config.json` —
nothing is fetched from the indexer repository at run time, and the version
that builds your index is the one your `package-lock.json` names.

```json
{
  "ci": { "forge": "github", "enrich": true, "autoMerge": false }
}
```

| Key | Effect |
|---|---|
| `forge` | `github` or `gitlab` — which pipeline is rendered |
| `defaultBranch` | The branch that deploys Pages, and that `publish: "default-branch"` releases from. Default `main` |
| `enrich` | `false` renders a pipeline that never goes online (pointers-only site) |
| `publish` | `tag` or `default-branch` adds the release pipeline for the [combined layout](#packages). Default `never` |
| `autoMerge` | `true` adds the squash-merge job for contributions the gate passed. GitHub only |
| `allowManualEdits` | `true` drops the drift check, and the workflows become yours to hand-edit |

`defaultBranch` exists because GitHub has no expression for the default
branch inside an `on:` trigger — it has to be a literal. A repository whose
trunk is `master` previously got workflows that were valid, committed, and
never fired. GitLab reads `$CI_DEFAULT_BRANCH` at run time and ignores the
key.

Edit the block, then re-render:

```console
$ npm run ci        # rewrite the workflow files
$ npm run ci:check  # what CI runs — exits 65 on drift
```

The generated `verify-ci` job runs `ci:check` on every push, so a
hand-edited workflow fails loudly rather than quietly forking the pipeline.
It tolerates a bumped `uses:` pin, so [Renovate][renovate] keeping the
actions current does not read as drift.

Picking up a renderer fix is an ordinary dependency bump — update
`@grimoire-rs/indexer` in `package.json`, run `npm install` and
`npm run ci`, commit the result. Never re-scaffold over your edits.

> **Coming from `0.2.x`?** Those scaffolds call reusable workflows in the
> indexer repository, which `0.3.0` deleted. The pinned refs still resolve
> — old tags are not removed — so an untouched index keeps working, and it
> breaks the moment something bumps the pin. Convert before that happens:
> add a `package.json` pinning `@grimoire-rs/indexer` (if the old scaffold
> has none) and run `npm run ci` to render the real workflows.

## Private and Internal {#private}

Nothing above assumes a public repository. Three shapes work:

- **Private repo, git transport.** Skip Pages entirely and hand consumers
  the clone URL: `index = "https://gitlab.example.com/platform/index.git"`.
  grim shallow-clones and walks the tree, authenticating through ambient
  git credentials — a credential helper or ssh agent. It never prompts.
- **Private repo, authenticated Pages.** [GitLab Pages access
  control][gl-pages-auth] restricts a site to project members once an
  administrator enables it instance-wide. GitHub's equivalent is narrower:
  publishing a Pages site privately [requires GitHub Enterprise
  Cloud][gh-pages-private], and only for a project site owned by the
  organization. Without it, a private repository's Pages site is still
  public — use the git transport instead.
- **Air-gapped.** `index-policy.json` bounds which registries entries may
  point at, so an internal index can refuse anything not on your mirror.

The corporate GitLab walkthrough — release mirrors for the grim binary
itself, auto-merge by group membership, publishers with zero forge
configuration — is [Self-Hosted GitLab Setup][gitlab].

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
[helm-repo]: https://helm.sh/docs/topics/chart_repository/
[gh-pages-private]: https://docs.github.com/en/pages/getting-started-with-github-pages/changing-the-visibility-of-your-github-pages-site
[gl-pages]: https://docs.gitlab.com/ee/user/project/pages/
[gl-pages-auth]: https://docs.gitlab.com/user/project/pages/pages_access_control/
[node]: https://nodejs.org/
[npm]: https://www.npmjs.com/package/@grimoire-rs/indexer
[renovate]: https://docs.renovatebot.com/modules/manager/npm/
[template]: https://github.com/grimoire-rs/index-template

<!-- internal -->
[config]: ./configuration.md
[skip-existing]: ./publishing.md#batch-publish-skip-existing
[consuming]: ./package-index.md#consuming
[spec-metadata]: ./package-index.md#spec-metadata
[announcing]: ./package-index.md#announcing
[gitlab]: ./self-hosted-gitlab.md
[upgrading]: #upgrading

<!-- grimoire -->
[index-site]: https://index.grimoire.rs
[wizard]: https://grimoire.rs/start.html
