#!/usr/bin/env bash
# Bootstrap a local OCI registry with the manual-rig sample catalog.
#
#   test/manual/scripts/bootstrap.sh
#
# Re-runnable: re-publishes the *current* catalog content with `--force`, so
# an edited artifact moves its exact-version tag to the new digest (identical
# content resolves to the same digest, an effective no-op).
#
# Publishes a small VERSION MATRIX (see step 4) so upgrade / `↑ outdated`
# states are exercisable: most artifacts ship a single 1.0.0, but a few carry
# extra versions (code-reviewer 1.0.0/1.1.0/1.2.0, commit-helper 1.0.0/2.0.0,
# rust-style 1.0.0/1.1.0) and the `starter-pack` bundle ships 1.0.0 plus a
# 2.0.0 whose member set adds AND removes entries. Each full-semver release
# cascades the floating :MAJOR/:MINOR/:latest tags forward, so versions MUST
# be published in ASCENDING order per artifact — the floating :1 the consumer
# project pins then lands on the highest version. A post-lock bump above the
# matrix top (scripts/release-update.sh) produces a genuine `↑ outdated` lock.
set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANUAL_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$MANUAL_DIR/../.." && pwd)"
CATALOG="$MANUAL_DIR/catalog"
# Own port — deliberately NOT 5000 (the pytest acceptance registry). See
# docker-compose.yml: sharing one registry polluted `grim search` here
# with the suite's throwaway `grim-test/*` repos.
REGISTRY="localhost:5050"
NS="grimoire"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# 1. Build the binary the pytest harness path expects, if missing/stale.
if [ ! -x "$REPO_ROOT/test/bin/grim" ] ||
    [ "$REPO_ROOT/Cargo.toml" -nt "$REPO_ROOT/test/bin/grim" ]; then
    log "building release grim"
    (cd "$REPO_ROOT" && cargo build --release --locked)
    cp "$REPO_ROOT/target/release/grim" "$REPO_ROOT/test/bin/grim"
fi
GRIM="$REPO_ROOT/test/bin/grim"

# 2. Ensure the registry is reachable (reuse a running one, else compose).
if ! curl -fsS "http://$REGISTRY/v2/" >/dev/null 2>&1; then
    log "starting registry via docker compose"
    docker compose -f "$MANUAL_DIR/docker-compose.yml" up -d
    for _ in $(seq 1 60); do
        curl -fsS "http://$REGISTRY/v2/" >/dev/null 2>&1 && break
        sleep 0.5
    done
fi
curl -fsS "http://$REGISTRY/v2/" >/dev/null 2>&1 ||
    {
        echo "registry not reachable at $REGISTRY" >&2
        exit 69
    }

# 3. Isolated GRIM_HOME for the rig.
export GRIM_HOME="$MANUAL_DIR/.grim-home"
export GRIM_DEFAULT_REGISTRY="$REGISTRY"
export GRIM_INSECURE_REGISTRIES="$REGISTRY"
mkdir -p "$GRIM_HOME"

release() { # <path> <repo-subpath> <name> <version>
    log "release $3:$4"
    # --force so re-seeding after editing the catalog moves the exact-version
    # tag to the new content. The rig owns this throwaway :5050 registry, so
    # overwriting an immutable version tag here is intended, not a footgun;
    # identical content still resolves to the same digest (an effective no-op).
    "$GRIM" release "$1" "$REGISTRY/$NS/$2/$3:$4" --force
}

# Publish one artifact at each of its versions, in ASCENDING order so the
# floating :MAJOR/:MINOR/:latest tags end up on the highest version.
#   <path> <repo-subpath> <name> <space-separated versions, ascending>
release_versions() {
    local path="$1" kind="$2" name="$3" versions_field="$4"
    local versions ver
    # Split the version field on whitespace regardless of the script's IFS.
    IFS=' ' read -r -a versions <<<"$versions_field"
    for ver in "${versions[@]}"; do
        release "$path" "$kind" "$name" "$ver"
    done
}

# 4. VERSION MATRIX. Keep it SMALL but covering: most artifacts ship one
#    1.0.0, a few carry extra versions for the upgrade / outdated demos.
#    Each record is `kind|name|path|space-separated versions (ascending)`.
#    A rule path is the index `<name>.md`; `grim release` packs the sibling
#    `<name>/` support dir automatically (the `rules/*.md` glob is
#    non-recursive, so a support file is never released as its own rule).
SKILL_MATRIX=(
    "skills|architecture-guide|$CATALOG/skills/architecture-guide|1.0.0"
    "skills|code-reviewer|$CATALOG/skills/code-reviewer|1.0.0 1.1.0 1.2.0"
    "skills|commit-helper|$CATALOG/skills/commit-helper|1.0.0 2.0.0"
    "skills|hello-world|$CATALOG/skills/hello-world|1.0.0"
)
RULE_MATRIX=(
    "rules|architecture-guide|$CATALOG/rules/architecture-guide.md|1.0.0"
    "rules|rust-style|$CATALOG/rules/rust-style.md|1.0.0 1.1.0"
    "rules|security-baseline|$CATALOG/rules/security-baseline.md|1.0.0"
)

# 4a. Publish skills.
for record in "${SKILL_MATRIX[@]}"; do
    IFS='|' read -r kind name path versions <<<"$record"
    release_versions "$path" "$kind" "$name" "$versions"
done

# 4b. Publish rules.
for record in "${RULE_MATRIX[@]}"; do
    IFS='|' read -r kind name path versions <<<"$record"
    release_versions "$path" "$kind" "$name" "$versions"
done

# 5. Publish bundles LAST — their members must already exist. The
#    `starter-pack` bundle ships two versions with differing member sets:
#      * 1.0.0 (starter-pack.toml):    code-reviewer + rust-style + security-baseline
#      * 2.0.0 (starter-pack-v2.toml): ADDS commit-helper, DROPS security-baseline
#    The published bundle name is the .toml file stem
#    (src/command/build.rs::read_bundle_members), so v2 is copied to a
#    mktemp `starter-pack.toml` first to publish under the SAME repo (else
#    :1 and :2 would be different repos and the upgrade demo would break).
bundle_tmp="$(mktemp -d)"
cleanup() { rm -rf "$bundle_tmp"; }
trap cleanup EXIT

release "$CATALOG/bundles/starter-pack.toml" bundles starter-pack 1.0.0

cp "$CATALOG/bundles/starter-pack-v2.toml" "$bundle_tmp/starter-pack.toml"
release "$bundle_tmp/starter-pack.toml" bundles starter-pack 2.0.0

log "done. Catalog published to $REGISTRY/$NS/{skills,rules,bundles}/*"
cat >&2 <<EOF

Next:
  source test/manual/scripts/env.sh
  grim search                       # browse the catalog (Version column = highest semver)
  grim tui                          # interactive browser (needs a TTY)
  cd test/manual/project
  grim lock && grim install         # materialize into .claude/
  grim status                       # all 'installed'

Outdated / update demo (lock at an OLD pin, then roll forward):
  # the project pins code-reviewer at the floating :1, so 'grim lock' here
  # records the newest published version (1.2.0). To force a real '↑ outdated'
  # lock, publish a version ABOVE the matrix top AFTER locking:
  test/manual/scripts/release-update.sh   # publishes code-reviewer 1.3.0
  grim status                             # code-reviewer -> 'outdated'
  grim update                             # rolls :1 -> 1.3.0; back to 'installed'

Bundle add/remove-on-upgrade demo:
  grim add bundle starter-pack localhost:5050/grimoire/bundles/starter-pack:1
  # resolves code-reviewer + rust-style + security-baseline
  grim add bundle starter-pack localhost:5050/grimoire/bundles/starter-pack:2
  # :2 ADDS commit-helper and DROPS security-baseline
EOF
