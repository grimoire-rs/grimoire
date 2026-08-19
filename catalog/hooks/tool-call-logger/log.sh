#!/bin/sh
# tool-call-logger — the reference `observer` payload.
#
# POSIX `sh`, not bash: a payload runs under whatever shell the client's
# environment provides, and grim promises nothing richer.
#
# The contract this demonstrates, in full:
#   1. grim writes one JSON envelope to stdin.
#   2. the payload writes one canonical JSON response to stdout.
#   3. the payload exits 0.
# An observer's response cannot change what happens, so this one answers
# `{}` — "no opinion" — which is also what every failure path degrades to.

set -u

# Where the line goes. Never the payload directory: grim hashes the payload
# tree, so writing there would make `grim status` report the artifact as
# locally modified. A temp path is the safe default; the environment
# variable exists so a human running the walkthrough can point it somewhere
# they can watch.
log="${GRIM_EXAMPLE_LOG:-${TMPDIR:-/tmp}/grim-tool-call-logger.log}"

# Drain the envelope. Nothing here needs to parse it — the facts this hook
# logs are already exported as flat scalars (see below) — but a payload that
# never reads its stdin makes grim's write fail with EPIPE.
cat >/dev/null

# grim exports a small allowlist of non-secret scalars. The envelope itself
# never travels through argv or the environment: it is stdin-only, because
# argv and environ are readable by any process at this privilege.
printf '%s client=%s tool=%s hook=%s tier=%s\n' \
    "${GRIM_HOOK_EVENT:-unknown}" \
    "${GRIM_HOOK_CLIENT:-unknown}" \
    "${GRIM_HOOK_TOOL:-none}" \
    "${GRIM_HOOK_NAME:-unknown}" \
    "${GRIM_HOOK_TIER:-unknown}" \
    >>"$log"

printf '%s\n' '{}'
