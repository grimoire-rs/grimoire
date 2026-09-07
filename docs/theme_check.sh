#!/usr/bin/env bash
# WP-Q acceptance assertions for C-019 (theme tokens), C-020 (dark-only, header
# order) and C-021 (clients matrix). Every check is one command with an exit
# code, exactly as the plan's Specify bullet states them.
#
# Run from anywhere; paths resolve against the repo root. Needs `docs/dist`,
# so run `task docs:build` (or `task docs:check`, which builds first) before it.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
theme=$root/docs/src/styles/theme.css
commands=$root/docs/dist/commands.html
clients=$root/docs/dist/clients.html
privacy=$root/docs/public/privacy.html
version=$(grep -m1 '^version' "$root/Cargo.toml" | cut -d'"' -f2)

fails=0

ok() { printf 'ok   %s\n' "$1"; }
no() {
    printf 'FAIL %s\n' "$1"
    fails=$((fails + 1))
}

check() {
    if eval "$2" >/dev/null 2>&1; then ok "$1"; else no "$1"; fi
}

for f in "$commands" "$clients"; do
    [ -f "$f" ] || {
        printf 'FAIL missing %s — run "task docs:build" first\n' "$f"
        exit 1
    }
done

# C-019 — the ground token, in source and in the stylesheet the page links.
check "C-019 theme.css sets --sl-color-bg to #161826" \
    "grep -cE -- '--sl-color-bg: *#161826' '$theme'"

# Astro minifies the emitted CSS, so the space after the colon is optional.
# Only the stylesheets commands.html actually links are searched — a rule
# sitting in an orphaned bundle would not reach the page.
sheets=$(grep -oE 'href="/_astro/[^"]+\.css"' "$commands" |
    sed 's/^href="//; s/"$//' | sort -u)
if [ -z "$sheets" ]; then
    no "C-019 commands.html links at least one /_astro stylesheet"
else
    hit=0
    for s in $sheets; do
        if grep -qE -- '--sl-color-bg: *#161826' "$root/docs/dist$s"; then hit=1; fi
    done
    if [ "$hit" = 1 ]; then
        ok "C-019 a linked stylesheet carries --sl-color-bg:#161826"
    else
        no "C-019 a linked stylesheet carries --sl-color-bg:#161826"
    fi

    # Print. Starlight's own print.css is linked with media="print" BEFORE the
    # chunk carrying theme.css's :root, and both are unlayered :root at equal
    # specificity — so the dark palette wins inside @media print unless
    # theme.css takes the tokens back later in its own file. Asserting the
    # source order inside the emitted chunk is what makes that load-bearing:
    # a print block that migrated above the :root would stop winning, silently.
    print_ok=0
    for s in $sheets; do
        sheet=$root/docs/dist$s
        grep -qE -- '--sl-color-bg: *#161826' "$sheet" || continue
        root_at=$(grep -boE -- '--sl-color-bg: *#161826' "$sheet" | head -1 | cut -d: -f1)
        print_at=$(grep -boE '@media print\{?[^}]*--sl-color-text: *#17181c' "$sheet" |
            head -1 | cut -d: -f1)
        if [ -n "$print_at" ] && [ "$print_at" -gt "$root_at" ]; then print_ok=1; fi
    done
    if [ "$print_ok" = 1 ]; then
        ok "C-019 the linked stylesheet's @media print sets --sl-color-text after its :root"
    else
        no "C-019 the linked stylesheet's @media print sets --sl-color-text after its :root"
    fi
fi

# C-020 — dark only. No picker markup, no runtime that could change the theme.
check "C-020 commands.html carries data-theme=\"dark\"" \
    "grep -qF 'data-theme=\"dark\"' '$commands'"
check "C-020 commands.html has no theme-toggle markup" \
    "! grep -qE 'starlight-theme-select|id=\"theme-icons\"|themeSelect' '$commands'"
# Every <script> in the page is either inline or `src`-referenced; a
# `localStorage` literal in the HTML can only have come from an inline one.
check "C-020 commands.html inline scripts never read localStorage" \
    "! grep -qF 'localStorage' '$commands'"

# C-020 header — brand anchor, then search, then the nav list, in DOM order.
pos() { grep -boF -m1 "$2" "$1" 2>/dev/null | head -1 | cut -d: -f1 || true; }
brand=$(pos "$commands" 'class="site-title')
search=$(pos "$commands" '<site-search')
# Markers stop before the closing quote: Astro appends its own scoped-style
# class (`class="site-title astro-6rvad3va"`), so a quoted marker never matches.
nav=$(pos "$commands" 'class="grim-nav')
if [ -n "$brand" ] && [ -n "$search" ] && [ -n "$nav" ] &&
    [ "$brand" -lt "$search" ] && [ "$search" -lt "$nav" ]; then
    ok "C-020 header DOM order brand($brand) < search($search) < nav($nav)"
else
    no "C-020 header DOM order brand(${brand:-?}) < search(${search:-?}) < nav(${nav:-?})"
fi

# C-021 — the matrix wrapper survives the port, and its cells stay left-aligned.
check "C-021 clients.html keeps class=\"matrix-table\"" \
    "grep -qF 'class=\"matrix-table\"' '$clients'"
check "C-021 theme.css scopes text-align:left under .matrix-table table" \
    "grep -A6 '^\.matrix-table table {' '$theme' | grep -qE 'text-align: *left'"

# C-018/C-020 — privacy.html no longer claims mdBook storage keys.
check "privacy.html mentions mdbook 0 times" \
    "[ \"\$(grep -ci mdbook '$privacy')\" = 0 ]"

# start.html and privacy.html are copied out of docs/public/ verbatim, so their
# version span keeps the literal `dev` that Header.astro uses only as a bare
# `astro dev` fallback. docs:build stamps the two copies; nothing else would
# notice if that step went missing again, and /start.html is the landing's
# third call to action.
for page in start privacy; do
    stamped=$(grep -o 'data-grim-version[^>]*>[^<]*' "$root/docs/dist/$page.html" |
        sed 's/.*>//')
    if [ "$stamped" = "$version" ]; then
        ok "dist/$page.html stamps the version span with $version"
    else
        no "dist/$page.html stamps the version span with $version (found '${stamped:-none}')"
    fi
done

# privacy.html promises "No third-party subresources, on either site", and that
# promise is the whole reason the page can claim no cookies and no analytics.
# Nothing else asserts it: docs:check gates URLs, declarations and tokens. One
# embed pointed at a CDN would break the notice silently, so the built tree is
# swept for any script, stylesheet, image or frame loaded from another origin.
# Anchors are excluded — the site links outward on purpose.
external=$(grep -rhoE '<(script|link|img|iframe|source|video|audio)[^>]*(src|href)="https?://[^"]*"' \
    "$root/docs/dist" --include='*.html' | grep -vE 'rel="(canonical|alternate)"' || true)
if [ -z "$external" ]; then
    ok "privacy.html no third-party subresources in docs/dist"
else
    no "privacy.html no third-party subresources in docs/dist"
    printf '%s\n' "$external" | sed 's/^/     /' | head -10
fi

printf '\n%s\n' "$fails failing check(s)"
[ "$fails" = 0 ]
