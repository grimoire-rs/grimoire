#!/bin/sh
# tool-logger — an `observer` payload for the manual rig.
#
# POSIX `sh`. Reads the envelope on stdin, appends one line, exits 0. An
# observer's stdout is dropped by grim, so this deliberately prints nothing:
# a payload that returned a verdict here would have it silently discarded,
# which is the observer contract and not a bug.
set -u

envelope=$(cat)

# The flat scalars arrive in the environment, so the common case needs no JSON
# parser. `GRIM_HOOK_DIR` is this artifact's own payload directory; grim
# expands the token in an argv handler (there is no shell to do it).
printf '%s\t%s\t%s\t%s bytes\n' \
    "${GRIM_HOOK_EVENT:-?}" \
    "${GRIM_HOOK_CLIENT:-?}" \
    "${GRIM_HOOK_TOOL:-?}" \
    "$(printf '%s' "$envelope" | wc -c | tr -d ' ')" \
    >> "${GRIM_HOOK_DIR:-.}/../tool-calls.log"
