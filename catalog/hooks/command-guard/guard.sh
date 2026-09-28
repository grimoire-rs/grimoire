#!/bin/sh
# command-guard — a `gatekeeper` payload, for demonstration only.
#
# POSIX `sh`, not bash. Reads the envelope on stdin, writes one canonical
# JSON response on stdout, exits 0 — always 0, even when refusing: a verdict
# travels as a document, never as an exit code. Some clients read a non-zero
# hook exit as "deny", so an internal error would silently become a refusal.
#
# WHAT THIS IS NOT: a security control. The gatekeeper tier is not a
# security boundary, and the check below is a substring match over the raw
# envelope — `rm  -rf /`, `rm -fr /`, and anything built at runtime all sail
# past it. It exists to show how a refusal reaches the client.

set -u

# The envelope is one JSON object. A real payload would parse it (jq, python,
# whatever the environment has); this one deliberately stays dependency-free,
# which is exactly why its matching is so weak.
envelope=$(cat)

case "$envelope" in
    *"rm -rf /"*)
        # `decision` + `reason`. grim projects both onto the invoking
        # client's own response shape — on Claude's PreToolUse that is
        # hookSpecificOutput.permissionDecision / permissionDecisionReason —
        # so the payload never needs to know which client asked.
        printf '%s\n' '{"decision":"deny","reason":"command-guard (a demonstration hook) refused a command containing the literal rm -rf /"}'
        exit 0
        ;;
esac

# The no-opinion answer, and the right default for anything permissive.
# NOT `{"decision":"allow"}`: on Claude and Copilot an explicit allow
# SUPPRESSES the client's own approval prompt, so it grants privilege rather
# than declining to object. An example must never hand that out.
printf '%s\n' '{}'
