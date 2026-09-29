---
title: Host a plugin marketplace
description: Turn a marketplace.toml into a git repository that Claude Code, Copilot, Codex, Qoder and Cursor users add by address, with a scheduled regenerate job, a required check and the token and review setup.
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

Goal: run a git repository that serves plugins to every supported
coding tool. Keep it current without hand edits. You declare the plugins
once in `marketplace.toml`. grim writes the files, and a scheduled job
regenerates them. A required check refuses anything that drifts from a
fresh export.

This page is for the curator. The people who add the repository read
[Use a plugin marketplace without grim](./use-a-marketplace.md).

## Before you start {#before-you-start}

You need `grim` 0.15.0 or later on your machine, a GitHub repository you
administer, and the registries your plugins come from. The workflows on
this page are GitHub Actions. [GitLab](#gitlab) has a component for the
same job.

Every workflow and script below is carried verbatim by
[grimoire-rs/e2e-marketplace][e2e], a working marketplace you can clone
and adapt. It registers as `grimoire-e2e` and offers two plugins built
from public registries.

## Lay out the repository {#layout}

A marketplace repository is one root. The curator owns the manifest and
the automation. grim owns everything it generates.

| Path | Owner | What |
|---|---|---|
| `marketplace.toml` | curator | The plugins and the `[marketplace]` table |
| `marketplace.lock` | grim | Pinned digests, written by `grim update --marketplace` and `grim export marketplace` |
| `.claude-plugin/marketplace.json`, `claude/<plugin>/` | grim | Claude Code |
| `.github/plugin/marketplace.json`, `copilot/<plugin>/` | grim | Copilot CLI and VS Code |
| `.agents/plugins/marketplace.json`, `codex/<plugin>/` | grim | Codex |
| `.qoder-plugin/marketplace.json`, `qoder/<plugin>/` | grim | Qoder |
| `.cursor-plugin/marketplace.json`, `cursor/<plugin>/` | grim | Cursor, only when you select it |
| `.github/workflows/marketplace.yml` | curator | Regenerate, deliver, keepalive |
| `.github/workflows/verify.yml` | curator | The required check |
| `scripts/` | curator | The gate, the verifier and the pull request body |
| `.github/CODEOWNERS` | curator | Review on everything a verifier trusts |

Add one line to `.gitignore`. grim stages every output in a hidden
directory and keeps a lock file in the root. Neither belongs in git.

```text
.grim-export*
```

If anyone edits on Windows, add a `.gitattributes` file with `* -text`.
grim writes `\n` line endings, and `core.autocrlf` would otherwise
rewrite the owned files and fail verification.

```text
* -text
```

## Declare the marketplace {#declare}

`marketplace.toml` lists the plugins. The `[marketplace]` table names the
marketplace and its owner in every client's file. This is the manifest of
the example repository:

```toml
[marketplace]
name = "grimoire-e2e"
owner = { name = "Grimoire" }
description = "End-to-end showcase of a grim-generated plugin marketplace"

[plugins.grim-essentials]
include = ["ghcr.io/grimoire-rs/bundles/grim-essentials:0"]
description = "Grimoire first-party skills: the grim CLI, AI-config authoring, and grim artifact authoring"

[plugins.hex]
include = ["ghcr.io/michael-herwig/arcana/hex:0"]
description = "Tiered multi-agent swarm orchestration: plan, execute, review, architect"
```

The [`[marketplace]` table](../configuration.md#marketplace-table) also
takes a `clients` list. Without it grim serves Claude Code, Copilot, Codex
and Qoder. Cursor is opt-in. The name is what consumers type after the
`@` in `<plugin>@<marketplace>`.

Generate the repository once by hand, then commit the result:

```sh
grim export marketplace
git add -A
git commit -m "feat: add the marketplace"
```

The first run refuses when a client directory such as `./claude/` already
holds files grim did not write. Read [the ownership
rules](../commands.md#export-marketplace-ownership) before you pass
`--force`.

## Know what grim owns {#ownership}

grim keeps no record of earlier runs. It decides what it may change from
the repository itself, by a convention you can rely on:

- A selected client owns `./<client>/` and its marketplace file. The file
  is grim's when it is absent or its `name` equals `[marketplace].name`.
- Inside an owned `./<client>/`, anything that is not a declared plugin is
  removed on the next run.
- Every other path is left alone, including the files of another tool that
  live beside yours.

The full rules, including what a refusal looks like, are under
[`grim export marketplace`](../commands.md#export-marketplace).

### Files that shadow yours {#shadowing}

Two clients read a file of their own before they read Claude's. A Droid
user is served by `.factory-plugin/marketplace.json` when it exists. A
Qoder user is served by `.qoder-plugin/marketplace.json`, even when your
repository does not select `qoder`. grim warns about both. Delete a
foreign copy, or select `qoder` and let grim own the file.

### Renaming the marketplace {#rename}

Changing `[marketplace].name` makes every existing marketplace file look
foreign. grim warns and then refuses. `--force` adopts the files under the
new name, and it deletes everything else in each adopted `./<client>/`.
Every consumer who added the marketplace must then add it again under the
new name. Rename only when you accept that.

## Change the marketplace {#change}

The normal change is one pull request. Edit `marketplace.toml`, run
`grim update --marketplace` and `grim export marketplace`, and commit the
manifest edit together with the regenerated files. The `verify` check
compares the committed files with a fresh export, so it passes on that pull
request, and merging it publishes.

```sh
grim update --marketplace
grim export marketplace
git add -A
git commit -m "feat: add the review plugin"
```

The regenerate workflow below is not the way changes land. Its daily
schedule refreshes pins: it resolves the floating tags again and opens a
pull request when a registry moved. It is also the fallback for a
manifest-only edit that an administrator merged past the failing check.
[The curator two-step](#two-step) explains that route.

## Regenerate on a schedule {#regenerate}

The regenerate workflow refreshes pins and repairs drift. It runs daily,
on a push to the default branch that touches `marketplace.toml`, and on
demand. It resolves the plugins again, exports, and opens one pull request
when anything changed. In the default mode, nothing reaches the default
branch until a person merges that pull request.

One run does these steps, in order:

1. Install grim.
2. Run `grim update --marketplace` and `grim export marketplace`, both with
   `--format json`, into the runner's temporary directory.
3. Run the policy gate. It reads `git status` before anything is staged.
   It fails the run on a changed path the export does not own, a symlink,
   an LFS path or a file over 10 MiB.
4. Stop when the tree is clean.
5. Commit as the bot, and push the fixed branch `grim/marketplace` with
   `--force-with-lease`.
6. Open the pull request, or rewrite the body of the one already open.

Carried verbatim by `grimoire-rs/e2e-marketplace`,
`.github/workflows/marketplace.yml`:

```yaml
name: marketplace

# Regenerates the marketplace from marketplace.toml and delivers the result as
# one pull request (mode merge-request, the default) or as a push to the
# default branch (mode push, only for a marketplace of trusted registries).
#
# Repository settings this workflow relies on:
#   - a protected default branch, the `verify` check required, CODEOWNERS
#     review required on .github/, scripts/, ocx.toml, ocx.lock,
#     marketplace.toml and marketplace.lock
#   - EITHER the repository variable APP_ID and the secret APP_PRIVATE_KEY of
#     a GitHub App (Contents and Pull requests: write, never Workflows), kept
#     in an Environment restricted to the default branch,
#     OR the fallback: Settings > Actions > General > "Allow GitHub Actions to
#     create and approve pull requests" with the default GITHUB_TOKEN. In an
#     organization repository the organization setting must allow it too;
#     when it does not, the run pushes the branch and then fails, naming both
#     fixes.
#     Pull requests opened with GITHUB_TOKEN start no `pull_request` run, so
#     this job verifies the tree itself before it pushes.

on:
  schedule:
    - cron: "17 4 * * *"
  push:
    branches: [main]
    paths: [marketplace.toml]
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: grim-marketplace
  cancel-in-progress: false

env:
  MANIFEST: ./marketplace.toml
  # merge-request | push
  MODE: merge-request
  BRANCH: grim/marketplace
  MAX_FILE_BYTES: "10485760"

jobs:
  regenerate:
    # Scheduled and pushed runs are always on the default branch; a manual run
    # from another branch would deliver against the wrong base.
    if: github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    timeout-minutes: 20
    # The App private key and the optional registry credential live only in this
    # environment (the `verify` workflow reads neither); restrict it to the
    # default branch in Settings > Environments.
    environment: marketplace
    permissions:
      contents: write # push the bot branch (or the default branch in push mode)
      pull-requests: write # open or rewrite the one bot pull request
    env:
      DEFAULT_BRANCH: ${{ github.event.repository.default_branch }}
    steps:
      - name: Token
        id: app
        if: ${{ vars.APP_ID != '' }}
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1 # v3.2.0
        with:
          app-id: ${{ vars.APP_ID }}
          private-key: ${{ secrets.APP_PRIVATE_KEY }}
          # Down-scoped to this repository; never `workflows`.
          permission-contents: write
          permission-pull-requests: write

      - name: Check out the default branch
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          # The token is handed to git only for the push below.
          persist-credentials: false

      - name: Install grim
        # The one step that differs from the documented workflow: grim comes
        # from ocx.toml (a dev build), not from a release tarball plus sha256.
        uses: ocx-sh/setup-ocx@25fa771f8572572dc64528db89560de68a163a0e # v1.4.0
        with:
          version: "0.6.3"
          # A job that holds a write token restores no shared cache.
          cache: "false"

      - name: Registry login
        if: ${{ vars.MARKETPLACE_REGISTRY != '' }}
        env:
          REGISTRY: ${{ vars.MARKETPLACE_REGISTRY }}
          REGISTRY_USER: ${{ vars.MARKETPLACE_REGISTRY_USER }}
          REGISTRY_PASSWORD: ${{ secrets.MARKETPLACE_REGISTRY_PASSWORD }}
        run: |
          set -eu
          printf '%s' "$REGISTRY_PASSWORD" |
            grim login "$REGISTRY" -u "$REGISTRY_USER" --password-stdin --allow-insecure-store

      - name: Regenerate
        run: |
          set -eu
          grim update --marketplace "$MANIFEST" --format json >"$RUNNER_TEMP/update.json"
          grim export marketplace --marketplace "$MANIFEST" --format json >"$RUNNER_TEMP/export.json"

      - name: Policy gate
        run: sh scripts/marketplace-gate.sh "$MANIFEST" "$RUNNER_TEMP/export.json"

      - name: Anything to commit
        id: status
        run: |
          set -eu
          if [ -z "$(git status --porcelain=v1 --untracked-files=all)" ]; then
            echo "changed=false" >>"$GITHUB_OUTPUT"
            echo "The marketplace is up to date."
          else
            echo "changed=true" >>"$GITHUB_OUTPUT"
          fi

      - name: Commit as the bot
        if: steps.status.outputs.changed == 'true'
        run: |
          set -eu
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git add -A
          git commit -q -m "chore(marketplace): regenerate the marketplace"

      - name: Set up Node (GITHUB_TOKEN fallback only)
        if: steps.status.outputs.changed == 'true' && steps.app.outcome == 'skipped'
        uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version: "22"

      - name: Verify the committed tree (GITHUB_TOKEN fallback only)
        # With the fallback token the pull request starts no `verify` run, so
        # the same check runs here, after the commit and before the push.
        if: steps.status.outputs.changed == 'true' && steps.app.outcome == 'skipped'
        run: |
          set -eu
          npm install -g @anthropic-ai/claude-code@2.1.284
          sh scripts/marketplace-verify.sh "$MANIFEST"

      - name: Deliver
        if: steps.status.outputs.changed == 'true'
        env:
          GH_TOKEN: ${{ steps.app.outputs.token || github.token }}
          GH_REPO: ${{ github.repository }}
        run: |
          set -eu
          git check-ref-format --branch "$BRANCH" >/dev/null
          [ "$BRANCH" != "$DEFAULT_BRANCH" ] || { echo "BRANCH must not be the default branch" >&2; exit 1; }

          auth=$(printf 'x-access-token:%s' "$GH_TOKEN" | base64 -w0)
          echo "::add-mask::$auth"
          export GIT_CONFIG_COUNT=1
          export GIT_CONFIG_KEY_0="http.https://github.com/.extraheader"
          export GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $auth"

          if [ "$MODE" = push ]; then
            git push origin "HEAD:refs/heads/$DEFAULT_BRANCH"
            exit 0
          fi
          [ "$MODE" = merge-request ] || { echo "MODE must be merge-request or push" >&2; exit 1; }

          # One bot branch, rewritten in place. The lease names the tip this
          # run saw (empty: the branch must not exist yet), so a concurrent
          # writer makes the push fail instead of being overwritten.
          seen=$(git ls-remote origin "refs/heads/$BRANCH" | cut -f1)
          git push --force-with-lease="refs/heads/$BRANCH:$seen" origin "HEAD:refs/heads/$BRANCH"

          sh scripts/pr-body.sh "$RUNNER_TEMP/update.json" "$RUNNER_TEMP/export.json" >"$RUNNER_TEMP/body.md"
          pr=$(gh pr list --head "$BRANCH" --base "$DEFAULT_BRANCH" --state open --json number --jq '.[0].number // empty')
          if [ -n "$pr" ]; then
            gh pr edit "$pr" --body-file "$RUNNER_TEMP/body.md"
          elif ! gh pr create --head "$BRANCH" --base "$DEFAULT_BRANCH" \
            --title "chore(marketplace): regenerate the marketplace" \
            --body-file "$RUNNER_TEMP/body.md"; then
            # The branch is pushed; only the pull request is missing. An
            # organization that forbids Actions to create pull requests
            # refuses GITHUB_TOKEN here, so fail loudly with both fixes.
            echo "::error title=Pull request not opened::Branch $BRANCH is pushed, but GitHub refused to open its pull request (reason above). Fix one: allow 'GitHub Actions to create and approve pull requests' in the organization settings and the repository settings, or install a GitHub App and set the APP_ID variable and APP_PRIVATE_KEY secret. Then re-run this workflow. To open the pull request by hand now: $GITHUB_SERVER_URL/$GH_REPO/compare/$DEFAULT_BRANCH...$BRANCH?expand=1"
            exit 1
          fi

  keepalive:
    # Scheduled workflows in a public repository are disabled after 60 days
    # without repository activity. Re-enabling the workflow is the documented
    # remedy; whether calling it early resets the timer is unverified, so the
    # README also names the manual re-enable.
    if: github.event_name == 'schedule'
    runs-on: ubuntu-latest
    timeout-minutes: 5
    permissions:
      actions: write # the only scope: PUT .../actions/workflows/{id}/enable
    steps:
      - name: Re-enable this workflow
        env:
          GH_TOKEN: ${{ github.token }}
          GH_REPO: ${{ github.repository }}
        run: gh api --method PUT "repos/$GH_REPO/actions/workflows/marketplace.yml/enable"
```

The workflow calls two scripts. The gate needs only POSIX `sh`, `git` and
`awk`. The pull request body script also needs `jq`, which the GitHub
runners carry. Carried verbatim by `grimoire-rs/e2e-marketplace`,
`scripts/marketplace-gate.sh`:

```sh
#!/bin/sh
# Policy gate for a regenerated marketplace working tree.
#
# Run from inside the repository, after `grim update --marketplace` and
# `grim export marketplace`, and BEFORE `git add` or any commit. It reads git's
# own status (nothing is staged first) and refuses the run when a changed path
# is not something the export owns.
#
#   marketplace-gate.sh <manifest> <export.json>
#
# <manifest>     the marketplace manifest the run used (its directory is the
#                output root; the lock is `<stem>.lock` beside it)
# <export.json>  the `grim export marketplace --format json` report
# MAX_FILE_BYTES  size cap per changed file (default 10485760)
#
# The allow-list is derived from the run, never from stored state:
#   - the manifest's lock path
#   - every `files[].path` of the report
#   - `<output root>/<client>/` for every `files[].client` (removed included)
#
# Exit: 0 clean (also when nothing changed), 1 policy violation, 2 bad input.
# Fails on: a path outside the allow-list, a symlink, a path whose `filter`
# attribute is `lfs`, a file over the size cap, a path containing a newline.
# POSIX sh; needs git, awk, sed, tr, wc. No jq, no bashisms (the GitLab
# component carries the same logic).
set -eu

die() {
    printf 'gate: %s\n' "$*" >&2
    exit 2
}

violations=0
fail() {
    printf 'gate: FAIL: %s\n' "$*" >&2
    violations=$((violations + 1))
}

[ "$#" -eq 2 ] || die "usage: marketplace-gate.sh <manifest> <export.json>"
manifest=$1
report=$2
max=${MAX_FILE_BYTES:-10485760}
case $max in
    '' | *[!0-9]*) die "MAX_FILE_BYTES must be a non-negative integer" ;;
esac
[ -f "$manifest" ] || die "manifest not found: $manifest"
[ -f "$report" ] || die "export report not found: $report"

abs_dir() { (cd "$1" 2>/dev/null && pwd -P); }

logical=$(pwd)
mdir=$(abs_dir "$(dirname "$manifest")") || die "manifest directory not found"
report=$(abs_dir "$(dirname "$report")")/$(basename "$report")
top=$(git rev-parse --show-toplevel) || die "not inside a git repository"
top=$(abs_dir "$top") || die "repository root not found"
cd "$top"

case $mdir in
    "$top") rel="" ;;
    "$top"/*) rel=${mdir#"$top"/}/ ;;
    *) die "manifest is outside the repository" ;;
esac
mbase=$(basename "$manifest")
lock="$rel${mbase%.*}.lock"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
tab=$(printf '\t')

# One line per allowed thing: `E<path>` exact, `P<dir>/` prefix.
printf 'E%s\n' "$lock" >"$tmp/allow"
awk '
    /^[ \t]*"files"[ \t]*:[ \t]*\[/ { f = 1; next }
    f && /^[ \t]*"(client|path)"[ \t]*:[ \t]*"/ {
        key = $0; sub(/^[ \t]*"/, "", key); sub(/".*$/, "", key)
        val = $0; sub(/^[ \t]*"[a-z]+"[ \t]*:[ \t]*"/, "", val); sub(/",?[ \t]*$/, "", val)
        print key "\t" val
    }
' "$report" >"$tmp/files"

nfiles=0
while IFS=$tab read -r key val; do
    case $val in
        *\\* | *'"'*) die "unsupported character in the export report: $val" ;;
    esac
    case $key in
        client)
            case $val in
                '' | *[!a-z]*) die "malformed client in the export report: $val" ;;
            esac
            printf 'P%s%s/\n' "$rel" "$val" >>"$tmp/allow"
            ;;
        path)
            case $val in
                "$top"/*) p=${val#"$top"/} ;;
                "$logical"/*) p=${val#"$logical"/} ;;
                *) die "report path is outside the repository: $val" ;;
            esac
            case /$p/ in
                */../* | */./* | *//*) die "unnormalised report path: $val" ;;
            esac
            printf 'E%s\n' "$p" >>"$tmp/allow"
            nfiles=$((nfiles + 1))
            ;;
    esac
done <"$tmp/files"
[ "$nfiles" -gt 0 ] || die "no files[] entries in the export report (fail closed)"

git status --porcelain=v1 -z --untracked-files=all --no-renames >"$tmp/status.raw" ||
    die "git status failed"
if [ "$(tr -cd '\n' <"$tmp/status.raw" | wc -c)" -ne 0 ]; then
    fail "a changed path contains a newline"
fi
tr '\0' '\n' <"$tmp/status.raw" >"$tmp/status"
if [ ! -s "$tmp/status" ]; then
    echo "gate: nothing changed"
    [ "$violations" -eq 0 ] || exit 1
    exit 0
fi

: >"$tmp/paths"
changed=0
while IFS= read -r line; do
    case $line in
        ?? | ??' ') die "malformed status record: $line" ;;
        ??' '?*) ;;
        *) die "malformed status record: $line" ;;
    esac
    p=${line#???}
    changed=$((changed + 1))
    allowed=0
    while IFS= read -r a; do
        case $a in
            E*) [ "${a#E}" = "$p" ] && allowed=1 ;;
            P*)
                case $p in
                    "${a#P}"*) allowed=1 ;;
                esac
                ;;
        esac
    done <"$tmp/allow"
    [ "$allowed" -eq 1 ] || fail "path outside the allow-list: $p"
    if [ -L "$p" ]; then
        fail "symlink: $p"
    elif [ -f "$p" ]; then
        size=$(wc -c <"$p")
        [ "$size" -le "$max" ] || fail "file over $max bytes ($size): $p"
    fi
    printf '%s\n' "$p" >>"$tmp/paths"
done <"$tmp/status"

# `filter=lfs` from any attributes source (.gitattributes, info/attributes,
# global): an LFS pointer would commit a blob the export never wrote.
git check-attr filter --stdin <"$tmp/paths" >"$tmp/attr" || die "git check-attr failed"
while IFS= read -r line; do
    case $line in
        *': filter: lfs') fail "LFS-managed path: ${line%: filter: lfs}" ;;
    esac
done <"$tmp/attr"

if [ "$violations" -ne 0 ]; then
    printf 'gate: %d violation(s) in %d changed path(s)\n' "$violations" "$changed" >&2
    exit 1
fi
printf 'gate: ok, %d changed path(s) all within the allow-list\n' "$changed"
```

Carried verbatim by `grimoire-rs/e2e-marketplace`, `scripts/pr-body.sh`:

```sh
#!/bin/sh
# Build the pull-request body from grim's JSON reports and print it.
#
#   pr-body.sh <update.json> <export.json>
#
# Every value that reaches the body comes from tool output that a registry
# publisher influences, so nothing is copied as-is: plugin names must match
# the plugin-name grammar (`[a-z0-9]+([.-][a-z0-9]+)*`, at most 64 characters),
# pins must be an OCI reference ending in `@sha256:<64 hex>`, digests, versions,
# kinds, clients and actions must match a closed shape. A row with any other
# value is dropped and counted, never rendered. The result is capped at 60000
# characters. The caller passes it with `gh pr create|edit --body-file`.
#
# Needs jq. POSIX sh.
set -eu

[ "$#" -eq 2 ] || {
    echo "usage: pr-body.sh <update.json> <export.json>" >&2
    exit 2
}
[ -f "$1" ] && [ -f "$2" ] || {
    echo "pr-body: report not found" >&2
    exit 2
}

jq -r -n --slurpfile upd "$1" --slurpfile exp "$2" '
def name_ok: type == "string" and length <= 64 and test("\\A[a-z0-9]+([.-][a-z0-9]+)*\\z");
def word_ok: type == "string" and test("\\A[a-z][a-z-]{0,15}\\z");
def digest_ok: type == "string" and test("\\Asha256:[0-9a-f]{64}\\z");
def opt_digest_ok: . == null or digest_ok;
def ver_ok: type == "string" and length <= 128 and test("\\A[0-9A-Za-z][0-9A-Za-z.+-]*\\z");
def pin_ok: type == "string" and length <= 512 and test("\\A[a-z0-9][a-z0-9._:/-]*@sha256:[0-9a-f]{64}\\z");
def short: if . == null then "-" else "`" + .[7:19] + "`" end;

($upd[0].items // []) as $u
| ($exp[0].items // []) as $e
| [$u[] | select(.action != "unchanged")] as $ur
| [$e[] | select(.action != "unchanged")] as $er
| [$ur[] | select((.plugin | name_ok) and (.name | name_ok) and (.kind | word_ok)
                  and (.action | word_ok) and (.old | opt_digest_ok) and (.new | opt_digest_ok))] as $uok
| [$er[] | select((.plugin | name_ok) and (.client | word_ok) and (.action | word_ok)
                  and (.version == null or (.version | ver_ok))
                  and ([.members[]? | .pinned | pin_ok] | all))] as $eok
| (($ur | length) - ($uok | length)) as $ud
| (($er | length) - ($eok | length)) as $ed
| ([ "## Marketplace regeneration",
     "",
     "`grim update --marketplace` and `grim export marketplace` regenerated the marketplace from `marketplace.toml`.",
     "**Merging publishes**: clients that follow this repository pick the change up on their next refresh.",
     "",
     "### Pin changes",
     "",
     (if ($uok | length) == 0 then "None."
      else "| Plugin | Kind | Artifact | Action | Old | New |\n|---|---|---|---|---|---|\n"
           + ([$uok[] | "| `\(.plugin)` | \(.kind) | `\(.name)` | \(.action) | \(.old | short) | \(.new | short) |"] | join("\n"))
      end),
     "",
     "### Plugin trees",
     "",
     (if ($eok | length) == 0 then "None."
      else "| Plugin | Client | Version | Action |\n|---|---|---|---|\n"
           + ([$eok[] | "| `\(.plugin)` | \(.client) | \(if .version == null then "-" else "`" + .version + "`" end) | \(.action) |"] | join("\n"))
      end),
     (if $ud + $ed > 0 then "\n\(($ud + $ed)) row(s) failed validation and are not shown; check the workflow log." else empty end),
     ""
   ] | join("\n")) as $body
| if ($body | length) > 60000
  then $body[0:59900] + "\n\n_Truncated: the report exceeds the pull-request body limit._\n"
  else $body end
'
```

### Install grim from a release {#install-step}

One step in both workflows differs from what you should copy. The example
repository consumes a development build of grim, so it installs through
[`ocx-sh/setup-ocx`][setup-ocx], which pulls the version that `ocx.toml`
and `ocx.lock` pin. Your marketplace uses a released grim instead. Replace
the `Install grim` step, and the `ocx.toml` it reads, with a step that
downloads the release archive and checks it against a literal `sha256`:

```yaml
      - name: Install grim
        env:
          GRIM_VERSION: v0.15.0
          # sha256 of the archive, from the .sha256 file beside the release asset
          GRIM_SHA256: "<64 hex characters>"
        run: |
          set -eu
          asset=grimoire-x86_64-unknown-linux-musl.tar.gz
          url="https://github.com/grimoire-rs/grimoire/releases/download/$GRIM_VERSION/$asset"
          curl --proto '=https' --tlsv1.2 -LsSf "$url" -o "$RUNNER_TEMP/$asset"
          echo "$GRIM_SHA256  $RUNNER_TEMP/$asset" | sha256sum -c -
          tar -xzf "$RUNNER_TEMP/$asset" -C "$RUNNER_TEMP"
          bin=$(find "$RUNNER_TEMP" -name grim -type f | head -n 1)
          install -D -m 0755 "$bin" "$HOME/.local/bin/grim"
          echo "$HOME/.local/bin" >>"$GITHUB_PATH"
          grim --version
```

Pinning the digest means a swapped archive fails the job. You move to a
new grim by editing two literals in a reviewed pull request. This step is
the only place the two forms differ. Delete `ocx.toml` and `ocx.lock` when
you make the swap, and drop them from `CODEOWNERS`.

## Verify every pull request {#verify}

The check runs on `pull_request`, never `pull_request_target`, and has no
`paths:` filter. A required check behind a path filter stays pending on
every pull request that skips it. The check regenerates the marketplace
inside the checkout and requires `git status` to show nothing. It then
runs `claude plugin validate` on the Claude marketplace file and every
`claude/<plugin>` tree.

Carried verbatim by `grimoire-rs/e2e-marketplace`,
`.github/workflows/verify.yml`:

```yaml
name: verify

# Regenerates the marketplace in the checkout and requires that git sees no
# difference, then runs `claude plugin validate`. The steps live in
# scripts/marketplace-verify.sh.
#
# Deliberately `pull_request` (never `pull_request_target`) and with no `paths:`
# filter: a required check behind a path filter stays Pending on every PR that
# skips it. Under `pull_request` the workflow, the scripts and the tool pins
# come from the PR head; CODEOWNERS review on those paths is what keeps a PR
# from swapping the verifier.
#
# No registry credential is used here: the job runs PR-head code, so any secret
# it read would be readable by anyone who can open a pull request from a branch.
# The showcase reads public packages only. Verifying a private registry needs a
# separate pull-only credential that you accept is readable by anyone with write
# access; see the README.

on:
  pull_request:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  verify:
    runs-on: ubuntu-latest
    timeout-minutes: 15
    permissions:
      contents: read
    env:
      MANIFEST: ./marketplace.toml
    steps:
      - name: Check out the PR head
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          persist-credentials: false

      - name: Install grim
        # The one step that differs from the documented workflow: grim comes
        # from ocx.toml (a dev build), not from a release tarball plus sha256.
        uses: ocx-sh/setup-ocx@25fa771f8572572dc64528db89560de68a163a0e # v1.4.0
        with:
          version: "0.6.3"

      - name: Set up Node
        uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version: "22"

      - name: Install claude
        # Exact version, headless: no login is needed for `plugin validate`.
        run: npm install -g @anthropic-ai/claude-code@2.1.284

      - name: Verify the marketplace
        run: sh scripts/marketplace-verify.sh "$MANIFEST"
```

The steps live in one script, so the regenerate workflow can run the same
check. Carried verbatim by `grimoire-rs/e2e-marketplace`,
`scripts/marketplace-verify.sh`:

```sh
#!/bin/sh
# Verification of a committed marketplace: regenerate it in place and require
# that git sees no difference.
#
#   marketplace-verify.sh [<manifest>]        (default ./marketplace.toml)
#
# Steps, in the checkout (nothing is copied):
#   1. No git submodule (mode 160000 entry) may sit at, beneath or above an
#      owned path: git reports neither changes inside a gitlink nor the
#      untracked files under it, so step 2 could not see a regenerated tree
#      there. Then `grim export marketplace --marketplace <manifest> --format json`
#      regenerates the marketplace files and trees where they live, from the
#      committed `<stem>.lock`. A curator edit to the manifest that the lock
#      does not cover shows up as a difference in step 2 (or fails here).
#   2. `git status` over the lock, every marketplace file and every client
#      directory the table defines (selected or not) must print nothing:
#      an edited, deleted, added or ignored owned file, a dropped client's
#      leftovers and a re-resolved lock all show up.
#   3. A client the report does not select must have no tracked marketplace
#      file and no tracked `<client>/` entry (a foreign or dropped tree would
#      otherwise pass step 2 untouched).
#   4. `claude plugin validate` (no --strict) over the Claude marketplace file
#      and every `claude/<plugin>` tree.
#
# Needs grim and claude on PATH, plus git, awk, tr, mktemp. POSIX sh.
set -eu

die() {
    printf 'verify: %s\n' "$*" >&2
    exit 1
}

manifest=${1:-./marketplace.toml}
[ -f "$manifest" ] || die "manifest not found: $manifest"
abs_dir() { (cd "$1" 2>/dev/null && pwd -P); }
mdir=$(abs_dir "$(dirname "$manifest")") || die "manifest directory not found"
manifest=$mdir/$(basename "$manifest")
top=$(abs_dir "$(git -C "$mdir" rev-parse --show-toplevel)") || die "not inside a git repository"
cd "$top"
case $mdir in
    "$top") rel="" ;;
    "$top"/*) rel=${mdir#"$top"/}/ ;;
    *) die "manifest is outside the repository" ;;
esac
lock="$rel$(basename "$manifest" | sed 's/\.[^.]*$//').lock"

work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/marketplace-verify.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

# The table: client, marketplace file. The tree directory is the client name.
table_file() {
    case $1 in
        claude) echo .claude-plugin/marketplace.json ;;
        copilot) echo .github/plugin/marketplace.json ;;
        codex) echo .agents/plugins/marketplace.json ;;
        qoder) echo .qoder-plugin/marketplace.json ;;
        cursor) echo .cursor-plugin/marketplace.json ;;
    esac
}
table_clients="claude copilot codex qoder cursor"

# The owned paths: the lock, every marketplace file, every client directory.
set -- "$lock"
for c in $table_clients; do
    set -- "$@" "$rel$(table_file "$c")" "$rel$c/"
done
printf '%s\n' "$@" >"$work/owned"

echo "verify: 1/4 no submodule on an owned path, regenerating in place"
git ls-files -s -z | tr '\0' '\n' | awk -F'\t' '
    NR == FNR { o = $0; sub(/\/$/, "", o); own[++n] = o; next }
    $1 ~ /^160000 / {
        p = $2
        for (i = 1; i <= n; i++)
            if (p == own[i] || index(own[i] "/", p "/") == 1 || index(p "/", own[i] "/") == 1) {
                print p; break
            }
    }
' "$work/owned" - >"$work/gitlinks"
if [ -s "$work/gitlinks" ]; then
    sed 's/^/verify:   /' <"$work/gitlinks" >&2
    die "a git submodule overlaps an owned path (paths above)"
fi
grim export marketplace --marketplace "$manifest" --format json >"$work/verify.json" ||
    die "grim export marketplace failed"

awk '
    /^[ \t]*"files"[ \t]*:[ \t]*\[/ { f = 1; next }
    f && /^[ \t]*"client"[ \t]*:[ \t]*"/ {
        v = $0; sub(/^[ \t]*"client"[ \t]*:[ \t]*"/, "", v); sub(/".*$/, "", v); print v
    }
' "$work/verify.json" | sort -u >"$work/selected"
[ -s "$work/selected" ] || die "the export report names no client (fail closed)"
while IFS= read -r c; do
    case $c in
        '' | *[!a-z]*) die "malformed client in the report: $c" ;;
    esac
    case " $table_clients " in
        *" $c "*) ;;
        *) die "the report names a client outside the table: $c" ;;
    esac
done <"$work/selected"

echo "verify: 2/4 git status over the owned paths"
git status --porcelain=v1 -z --untracked-files=all --ignored=matching -- "$@" >"$work/status"
if [ -s "$work/status" ]; then
    tr '\0' '\n' <"$work/status" | sed 's/^/verify:   /' >&2
    die "the committed marketplace differs from a fresh export (paths above)"
fi

echo "verify: 3/4 unselected clients carry no tracked content"
for c in $table_clients; do
    grep -qx "$c" "$work/selected" && continue
    git ls-files -z -- "$rel$(table_file "$c")" "$rel$c/" >"$work/tracked"
    if [ -s "$work/tracked" ]; then
        tr '\0' '\n' <"$work/tracked" | sed 's/^/verify:   /' >&2
        die "client '$c' is not selected but has tracked content (paths above)"
    fi
done

echo "verify: 4/4 claude plugin validate"
if grep -qx claude "$work/selected"; then
    claude plugin validate "$rel.claude-plugin/marketplace.json" ||
        die "claude plugin validate failed on the marketplace file"
    for d in "$rel"claude/*/; do
        [ -d "$d" ] || continue
        claude plugin validate "${d%/}" || die "claude plugin validate failed on ${d%/}"
    done
else
    echo "verify: claude is not selected, nothing to validate"
fi
echo "verify: ok"
```

The script catches an edited, deleted, added or ignored file under an
owned path. It also catches a marketplace file or tree for a client you
did not select, which a plain `git status` would miss. It pins `claude`
to an exact version and needs no login to validate. One gap remains: a
hand edit to the `declaration_hash` or `generated_by` line of
`marketplace.lock` changes no output, so it passes.

The check proves that the output renders the committed inputs. It does not
prove where the pins came from. A pull request can swap a pin's repository
or digest, regenerate the trees to match, and pass. The `marketplace.lock`
diff is the review surface, so put it under [code owners](#protect).

### The curator two-step {#two-step}

A pull request that edits only `marketplace.toml` fails this check. The
committed files no longer match a fresh export, and that is intended. The
regenerate workflow runs from the default branch, so it cannot repair a
pull request that has not merged.

The normal way through is to commit the regenerated files in the same pull
request, as [Change the marketplace](#change) shows. The fallback is for
when you cannot regenerate. Let an administrator merge the manifest edit
past the failing check. The push then starts the regenerate workflow, and
its pull request lands the generated files. Merge that second pull request
to publish.

## Set up the tokens {#tokens}

The regenerate job needs a token that can push a branch and open a pull
request. Two forms work, and they differ in one way that matters.

| Token | Setup | Effect |
|---|---|---|
| GitHub App (preferred) | Repository variable `APP_ID`, secret `APP_PRIVATE_KEY`, in an Environment named `marketplace` | The bot's pull request starts the `verify` check like any other |
| `GITHUB_TOKEN` | Settings, Actions, General, **Allow GitHub Actions to create and approve pull requests** | A pull request opened with this token starts no `pull_request` run |

Give the App Contents and Pull requests write access, and never
Workflows. The workflow scopes the token to this repository.

With `GITHUB_TOKEN`, the required check stays pending on the bot's pull
request, because GitHub starts no run for it. The regenerate job
compensates. After it commits and before it pushes, it runs the same
verification script on the committed tree. You then have two choices. Do
not mark the check required, or close and reopen the bot's pull request,
which is a human event that runs it.

In an organization repository the organization setting overrides the
repository one. When the organization forbids Actions to create pull
requests, `GITHUB_TOKEN` cannot open the bot's pull request at all. The
workflow then pushes the bot branch and fails with one error that names
both fixes. Allow the setting at both levels, or set up the GitHub App.

For plugins from a private registry, set the variables
`MARKETPLACE_REGISTRY` and `MARKETPLACE_REGISTRY_USER`. Set the secret
`MARKETPLACE_REGISTRY_PASSWORD` too. Keep all three in the `marketplace`
Environment, where only the regenerate job reads them.

The verify workflow above reads none of them. It runs the code of the pull
request under test. A secret it used would be readable by anyone who can
open a pull request from a branch.

To verify a marketplace with private members, add a login step to the
verify workflow with a separate pull-only read credential. Anyone with write
access can read that credential. Never reuse the regenerate one.

## Protect the repository {#protect}

The automation is only as trustworthy as the branch it merges into. Set
these before you announce the address:

- Protect the default branch and require the `verify` check.
- Require review from code owners, and dismiss stale approvals.
- Keep the bot off every bypass list.
- Restrict the `marketplace` Environment to the default branch, in
  Settings, Environments.

Then add a `.github/CODEOWNERS` that covers what a verifier trusts:

```text
/.github/**   @your-org/marketplace-curators
/scripts/**   @your-org/marketplace-curators
/ocx.toml     @your-org/marketplace-curators
/ocx.lock     @your-org/marketplace-curators
/marketplace.lock  @your-org/marketplace-curators
/marketplace.toml  @your-org/marketplace-curators
```

This closes a real hole. Under `pull_request`, the workflow files, the
scripts and the tool pins come from the pull request head. A pull request
can therefore replace the verifier with one that always passes. Required
code-owner review on those paths is the only mitigation, so treat it as a
requirement, not a suggestion. The last two lines cover the inputs: a
reviewer reads the `marketplace.lock` diff to see which pins changed.

The review does not defend against a compromised plugin in an upstream
registry. It shrinks that exposure window and is not a control against it.

## Publish and maintain {#publish}

Merging the bot's pull request publishes. Clients that follow the default
branch pick the change up on their next refresh. Tag a release with `git
tag` when you want consumers to pin a version. They add the address with
`#<tag>`, as the [consumer guide](./use-a-marketplace.md#pin-and-update)
shows.

### Let consumers clone less {#sparse}

A client clones the whole repository, including the trees of the other
tools. Consumers who want only their own tree can clone sparsely. The
[consumer guide](./use-a-marketplace.md#claude-code) gives the paths per
tool. Keep the plugin trees small, and let the gate's size cap catch a
stray large file.

### Removing a plugin {#removal}

Delete the plugin from `marketplace.toml`. The next run removes its tree
and drops it from every marketplace file. Claude Code users then see the
plugin as not found in the marketplace, and they keep the copy they
already installed.

Claude's `forceRemoveDeletedPlugins` is a top-level field of
`marketplace.json` that a curator sets. It would uninstall the plugin on
their machines. grim does not write that field yet. Tell consumers when you
remove something.

### Push mode {#push-mode}

`MODE: push` in the regenerate workflow commits straight to the default
branch and skips the pull request. Use it only for a marketplace whose
plugins all come from trusted private registries, because nobody reviews the
change before it publishes. The token must be allowed to push to the
protected branch, which conflicts with keeping the bot off every bypass
list. Keep the default `merge-request` mode otherwise.

### Keeping the schedule alive {#keepalive}

GitHub disables a public repository's scheduled workflows after 60 days
without activity. The `keepalive` job in the regenerate workflow calls the
enable endpoint, with only the `actions: write` scope. Whether that resets
the timer is unverified. If the schedule stops, re-enable `marketplace`
from the Actions tab.

## Tools that read Claude's file {#best-effort}

Junie, OpenClaw and Droid own no marketplace file. They read the Claude
Code one, `.claude-plugin/marketplace.json`, so grim serves them on a best
effort basis and generates nothing of their own. Naming one of them in
`clients` exits `65` and tells you to select `claude`. Droid reads
`.factory-plugin/marketplace.json` first when it exists, so a repository
that holds one [shadows the Claude file](#shadowing) for Droid users.

## Host on GitLab {#gitlab}

The [`grimoire-components`][components] project ships a `marketplace`
component for GitLab CI. It runs the same flow: install grim from a
release archive against a literal `sha256`, regenerate, gate, commit as
the bot and open one merge request. A verification job runs on merge
requests. The component's README lists its inputs and the token rules.
The verification job holds no credential and never logs in, so the template
cannot yet verify against a private registry.

Merge request mode needs a token with the Developer role and the
`write_repository` scope. Store it as a protected and masked variable, so
a branch pipeline cannot read it. Only push mode needs a Maintainer token, so
prefer merge requests for the same reason as above.
GitLab injects protected variables on every protected branch, so keep
protected branches to maintainers or scope the variables to the default
branch's environment.

## Related guides {#related}

- [Use a plugin marketplace without grim](./use-a-marketplace.md) is the
  consumer side of this repository.
- [`grim export marketplace`](../commands.md#export-marketplace) documents
  the flags, the layout and the exit codes.
- [The `[marketplace]` table](../configuration.md#marketplace-table) lists
  every key.
- [Hand a team a plugin](./team-plugin.md) exports one plugin file when a
  whole repository is more than you need.

<!-- external -->
[e2e]: https://github.com/grimoire-rs/e2e-marketplace
[setup-ocx]: https://github.com/ocx-sh/setup-ocx
[components]: https://gitlab.com/grimoire-rs/components
