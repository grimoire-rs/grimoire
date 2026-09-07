---
title: Browse the index
description: Find and install an artifact from the terminal, the website, or VS Code.
---
<!-- doc_type: how-to -->
<!-- doc_tier: first-steps -->

Find one artifact worth installing and get it into a project. There are three
ways in, and this page walks all of them: the terminal browser, the website,
and the VS Code extension. Each one ends with an artifact installed.

## See it run

<div data-cast="/casts/browse-tui.cast" data-cast-poster="npt:0:03"></div>
<noscript><a href="/casts/browse-tui.cast">Download the recording</a></noscript>

## Browse in the terminal

`grim tui` opens a browser over every configured source. It draws a tree
grouped by browse source, the live install state of each row, and a detail
pane for the row you are on.

1. Launch it from inside the project you want to install into.

   ```sh
   grim tui
   ```

2. Press `/` and type part of a name to narrow the list.
3. Move to the row you want with the arrow keys.
4. Press `i` to install it.

The row you installed changes state in place, so you see the result without
leaving the browser. The browser has its own state words, and they are not the
set `grim status` prints. The two share `installed`, `outdated` and `modified`
and nothing else. Every word from both is defined once at
[artifact states](./commands.md#artifact-states).

## Key map

This key map belongs to `grim tui` alone, and pressing `?` shows it inside the
app. The website and the VS Code extension have no key map.

| Key | What it does |
|---|---|
| `↑` / `↓` | Move the selection |
| `j` / `k` | Scroll the detail pane by line |
| `pgup` / `pgdn` | Scroll the detail pane by page |
| `space` | Mark or unmark a row |
| `a` | Mark all visible rows |
| `c` | Clear the marks |
| `i` | Install the marked set, or the selection |
| `u` | Update the marked set, or the selection |
| `d` | Uninstall the marked set, or the selection |
| `v` | Pick a specific version for the selected row |
| `o` | Open the selected entry's repository URL |
| `g` | Toggle scope between project and global |
| `t` | Toggle tree and flat view |
| `→` / `←` | Expand and collapse a group, in tree view. On a collapsed group or a leaf, `←` jumps to the parent |
| `z` | Fold the whole tree, in tree view. Expands everything, or collapses back to the [`expand_levels`](./configuration.md#options-tui) depth |
| `enter` | Open the detail pane, or fold a group in tree view. The only way to retry a readme or changelog that failed to load |
| `tab` / `shift-tab` | Switch the detail panel between overview, readme and changelog |
| `h` | Show or hide deprecated artifacts |
| `r` | Refresh the catalog |
| `/` | Start a search |
| `esc` | Quit, on the first press |
| `?` | Open this help |
| `q` | Quit |

## Badge legend

Every row carries a badge for its install state. A group row carries the badge
of the worst state below it, so a collapsed tree still shows what needs
attention. Each state name is defined once at
[artifact states](./commands.md#artifact-states).

| Badge | Means |
|---|---|
| `✓` | Installed |
| `◆` | Via bundle |
| `·` | Not installed |
| `↑` | Outdated |
| `✱` | Modified |
| `+` | Pending |
| `✘` | Integrity missing |
| `▾` / `▸` | An expanded or collapsed group, in tree view |
| `▣` | A marked row, or a group whose every descendant is marked |
| `▨` | A group with only some descendants marked |
| `3/8` | How many of a group's artifacts are installed |

## Index filters

A filter narrows one source down to the repositories your team should see. A
shared index of two thousand packages can then show a team the forty it cares
about.

A source entry may declare `include` and `exclude` glob patterns. Both
`grim search` and `grim tui` then show only the repositories those patterns
admit. A source whose filter admits nothing keeps its tree root and rolls up
as `0/0`.

A filter is read once at startup. Editing it while the browser is open changes
nothing until you quit and reopen.

The `--registry` flag is not a filter. It collapses the browse to the sources
it names, and it applies no filter at all.

A filter that admits nothing says so. `grim search` prints
`registry '<name>': filter admitted 0 of N repositories` to stderr, with a link
to the pattern rules. The TUI cannot print it, because its log goes to
`$GRIM_HOME/tui.log` while the alternate screen is up. What the browser shows
instead is a `filtered:` clause in its status line.

If a source looks empty in the TUI, run `grim search` with no query to see the
reason. The diagnostic is reported only for a bare listing, not for a search
term.

The full mechanics live in [configuration](./configuration.md). For the reason
to narrow what you see in the first place, read
[registries](./guides/registries.md).

## Browse on the website

[index.grimoire.rs](https://index.grimoire.rs) is the same catalog in a
browser. It is also the browse fallback `grim` uses when nothing else is
configured, so it is what you are searching before you configure anything.

1. Open [index.grimoire.rs](https://index.grimoire.rs) and find an artifact.
2. Follow its deep link into VS Code, then install it there.

An index site can also offer an `add-registry` deep link, which hands the
source to the extension instead of a single artifact.

## Browse in VS Code

The extension is `grimoire-rs.grimoire` on the marketplace. Its **Browse** tab
searches every configured source and puts a chip row of kinds above the
results. **Installed** lists what this workspace already has, and **Updates**
lists what has moved ahead of your lock.

1. Install `grimoire-rs.grimoire` from the marketplace.
2. Trust the workspace, because install actions run `grim`.
3. Open the **Browse** tab and search for an artifact.
4. Narrow the results with a kind chip.
5. Install it, then confirm it under **Installed**.

`Grimoire: Show Info` reports which `grim` the extension would spawn, how it
resolved that path, and what version it is.

## When grim is not on your PATH

Install actions run `grim`, so the extension has to find a binary first. When
`grim` is not on your `PATH`, the extension offers to download the release
from GitHub and verifies its checksum. Run `Grimoire: Install grim` to take
the offer. The extension's own README carries the details, so this page does
not repeat them.

## Next steps

Once you have found something worth installing,
[write your first skill](./first-skill.md) by hand and install it from a
path. To put a company registry beside the public one,
read [registries](./guides/registries.md). To decide whether an artifact lands
in one project or on the whole machine, read
[scopes and clients](./guides/scopes-and-clients.md).
