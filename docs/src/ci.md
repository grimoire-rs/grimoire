# Publishing from CI

Publishing by hand works until the second contributor bumps a version and
forgets to run `grim publish`. The natural home for publishing is CI: every
merge to the default branch (or every tag) re-publishes the manifest, and
[skip-existing](./publishing.md#batch-publish-skip-existing) makes the re-run idempotent —
unchanged versions are no-ops, bumped versions push.

Grimoire ships first-party CI integrations for both major forges: a
[GitHub Action][setup-grimoire] and [GitLab CI/CD components][gl-components].
Both install the released `grim` binary (checksum-verified), and the GitLab
side adds a complete publish job as a one-line include. This page shows the
full setup for each — publishing to a registry, announcing to the
[package index](./package-index.md), and the tokens each step needs.

## GitHub Actions {#github-actions}

Two credentials are involved, and keeping them apart is the whole trick:

| Step | Credential | Why |
|---|---|---|
| `grim publish` to GHCR | `GITHUB_TOKEN` with `packages: write` | Registry push stays inside the repo's own permissions |
| `grim publish --announce` | A separate token that can push to the index repository | `GITHUB_TOKEN` is repo-scoped — it cannot open a PR on [grimoire-rs/index][index-repo] |

A minimal publish workflow:

```yaml
name: Publish
on:
  push:
    branches: [main]

permissions:
  contents: read
  packages: write

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: grimoire-rs/setup-grimoire@v1
      - name: grim login
        env:
          REGISTRY_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          echo "$REGISTRY_TOKEN" | grim login ghcr.io -u "$GITHUB_ACTOR" \
            --password-stdin --allow-insecure-store
      - name: Wire announce token into git
        env:
          GH_TOKEN: ${{ secrets.INDEX_ANNOUNCE_TOKEN }}
        run: gh auth setup-git
      - name: Publish
        env:
          GH_TOKEN: ${{ secrets.INDEX_ANNOUNCE_TOKEN }}
        run: grim publish --announce
```

`--announce` clones the index repository, writes your `metadata.json`
pointers, and opens the pull request directly through the GitHub REST API
— no `gh` CLI involved. The token comes from `GRIM_ANNOUNCE_TOKEN` (always
wins) or, as here, from `GH_TOKEN`/`GITHUB_TOKEN` when the index lives on
the same GitHub host the CI runs on. The git push itself uses ambient git
credentials — on GitHub runners, `gh auth setup-git` or a credential
helper fed from the same token. The announce credential must be able to
**push a branch** — either straight to the index repository, or, when it
lacks write access there, to a fork grim creates or reuses on your behalf:

- **Your own or your organization's index** — a fine-grained PAT or GitHub
  App installation token with `contents` + `pull-requests` write on the
  index repository. This is exactly how the [first-party catalog
  publishes][publish-catalog].
- **The public index, without write access** — no extra setup needed. grim
  detects the missing push permission, forks [grimoire-rs/index][index-repo]
  into the token's account (creating the fork through the GitHub API, or
  reusing one that already exists — including a fork renamed since it was
  created), pushes the announce branch there, and opens the pull request
  cross-repository against the upstream index. The
  [auto-merge validation](./package-index.md#spec-validation) checks the
  PR author, so the PR must come from you either way. Set `[announce] fork
  = "never"` (or the legacy `false`) to disable this and always push
  directly instead, which fails the same way it always has without write
  access; `"always"` goes the other way and forks even where the token
  *could* push, so an announce from a maintainer's own credential still
  arrives as a reviewable PR. A token that cannot
  create the fork — a fine-grained PAT scoped to a single repository, say —
  or a fork that never becomes readable/pushable in time hard-errors here
  (exit 69: the fork could not be created, verified, or readied; the
  packages are already published, only the announce needs a retry).
  Recover by hand: fork the index yourself, point `[announce] repository`
  (or `--announce-repo`) at your **fork**, and open the pull request from
  the branch banner GitHub shows on it.

Skipping `--announce` needs no extra token at all — publish is fully
self-contained on `GITHUB_TOKEN`.

## GitLab CI/CD {#gitlab}

On GitLab the same pipeline is a component include. The
[`grimoire-rs/components`][gl-components] catalog project provides two
components:

| Component | What it adds |
|---|---|
| `setup` | A hidden `.grim-setup` job that installs `grim` — see [Installation](./installation.md#gitlab-ci) |
| `publish` | A complete `grim-publish` job: install, `grim login`, `grim publish`, optional announce |

The publish component defaults to the **project's own GitLab container
registry** using the job token — zero secrets for the registry side:

```yaml
# .gitlab-ci.yml
include:
  - component: gitlab.com/grimoire-rs/components/publish@1.1.0
    inputs:
      stage: deploy
```

```toml
# publish.toml
registry = "registry.gitlab.com"
repository_prefix = "your-group/your-project"

[skills.my-skill]
version = "1.0.0"
```

The [GitLab container registry][gitlab-registry] requires every image to
live under a group-and-project path — `repository_prefix` handles that
(details: [Repository namespace](./publishing.md#batch-publish-namespace)).

> `include: component:` only resolves components hosted on the **same
> GitLab instance**. On self-managed GitLab, [mirror the components
> project][gl-mirror] into your instance first (or copy the two template
> files — they are self-contained).

### Announcing to the public index {#gitlab-announce-public}

Announcing from GitLab CI to the GitHub-hosted public index crosses
forges, so the job needs a GitHub token — the GitLab CI environment
deliberately contributes nothing when the index host differs from the CI
server host. Hand the token to the component: it exports it as
`GRIM_ANNOUNCE_TOKEN` and installs it as the git credential for the push,
and grim opens the pull request via the GitHub REST API — no `gh` CLI
involved:

```yaml
include:
  - component: gitlab.com/grimoire-rs/components/publish@1.1.0
    inputs:
      announce: true
      announce_token: $INDEX_ANNOUNCE_TOKEN   # masked CI/CD variable
```

The same write-access rule as on GitHub Actions applies: with a token that
can push to the index repository, grim pushes there directly by default;
without one, it automatically forks the index into the token's account,
pushes the branch to the fork, and opens the PR cross-repository — no
`announce_repo` override needed. Fork reuse is identity-based and
tolerant of a rename, scoped to your **personal namespace** on GitLab (a
fork later moved into a group namespace is not reused — see [Announcing
Packages](./package-index.md#announcing) for the full behavior, including
the bounded wait while a newly created fork becomes ready). Set
`[announce] fork = "never"` (or the legacy `false`) in `publish.toml` to
fall back to the manual workaround (`announce_repo:
https://github.com/<you>/index`, opening the PR from the fork's branch
banner) for a token that cannot create forks — or `"always"` to fork even
when the token can push.

### Announcing to a self-hosted index {#gitlab-announce-self-hosted}

A company index is [just a git repository](./package-index.md#self-hosting)
— host it on the same GitLab instance and announce with a [project access
token][gl-pat] (`api` scope, so grim can open the merge request through
the GitLab API; with `write_repository` only, grim falls back to git push
options and finally to the pushed branch):

```yaml
include:
  - component: gitlab.com/grimoire-rs/components/publish@1.1.0
    inputs:
      announce: true
      announce_repo: https://gitlab.example.com/platform/index.git
      announce_token: $INDEX_ANNOUNCE_TOKEN
```

Because the index host matches the CI server host, grim auto-detects the
GitLab forge and API from the CI environment — no `[announce] forge` or
`api_url` config needed. Re-announcing the same content is detected as
up-to-date; changed content force-updates the same deterministic
`announce/<ns>-<hash>` branch (and its open MR) instead of littering new
ones. The self-hosted index can run the same
[validation and auto-merge](./package-index.md#spec-validation) gate as
the public one — the index repo ships a `.gitlab-ci.yml` for exactly
that; setup in [Self-Hosted GitLab Setup](./self-hosted-gitlab.md).

#### The job token carries the push {#gitlab-announce-job-token}

Even without an announce token, the **push transport** works on a
same-host index: when grim runs inside GitLab CI (`GITLAB_CI` set,
`CI_JOB_TOKEN` present) and the index host equals `CI_SERVER_HOST`, it
hands git a fallback credential — `gitlab-ci-token:$CI_JOB_TOKEN` via an
inline credential helper that reads the token from the job environment.
The token value never enters grim, a command line, or the disk, and it is
never used for the MR API (it cannot open MRs). Ambient git credentials
always win; the job token only answers when nothing else does.

The prerequisite lives on the **index project**: its
[job token permissions][gl-jobtoken] must allow the publishing project —
add it to the allowlist and enable *Allow Git push requests to the
repository* (GitLab 17.2+). With that in place, `announce: true` needs no
`announce_token` at all: grim pushes the topic branch on the job token
and opens the MR via push options where the server permits, else leaves
the branch for a manual MR.

The outcome is machine-readable: `grim publish --announce --format json`
reports `{outcome, branch, url, fork}` under `announce`
([Report output](./publishing.md#batch-publish-report)) — a downstream
job reads the branch from there instead of grepping stderr. Grim also
runs fine in GitLab's `HOME`-less step environments: registry
credentials degrade to anonymous unless `DOCKER_CONFIG` (or a
`grim login` step) provides them, and the announce push needs no
`~/.gitconfig`. Set `GRIM_HOME` when you want grim's data root somewhere
other than the working directory.

Consumers then wire the index into their config as usual:

```toml
[[registries]]
alias = "platform"
index = "https://gitlab.example.com/platform/index.git"
```

See [Consuming an Index](./package-index.md#consuming) for the transports
and caching behavior.

<!-- external -->
[gitlab-registry]: https://docs.gitlab.com/ee/user/packages/container_registry/
[gl-jobtoken]: https://docs.gitlab.com/ci/jobs/ci_job_token/
[gl-components]: https://gitlab.com/grimoire-rs/components
[gl-mirror]: https://docs.gitlab.com/user/project/repository/mirror/
[gl-pat]: https://docs.gitlab.com/user/project/settings/project_access_tokens/
[index-repo]: https://github.com/grimoire-rs/index
[publish-catalog]: https://github.com/grimoire-rs/grimoire/blob/main/.github/workflows/publish-catalog.yml
[setup-grimoire]: https://github.com/grimoire-rs/setup-grimoire
