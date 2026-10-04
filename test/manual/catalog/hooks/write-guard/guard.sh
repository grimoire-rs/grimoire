#!/bin/sh
# write-guard — a `gatekeeper` payload for the manual rig.
#
# Exits 0 on every path, including the refusal: a verdict travels as a JSON
# document, never as an exit code. Some clients read a non-zero hook exit as
# "deny", so an internal error would silently become a refusal.
set -u

envelope=$(cat)

case "$envelope" in
    *.env*)
        printf '%s\n' '{"decision":"deny","reason":"write-guard (a manual-rig demonstration hook) refused a Write mentioning .env"}'
        exit 0
        ;;
esac

# The no-opinion answer. NOT `{"decision":"allow"}` — on Claude and Copilot an
# explicit allow SUPPRESSES the client's own approval prompt, so it grants
# privilege rather than declining to object.
printf '%s\n' '{}'
