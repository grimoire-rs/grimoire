# Research: Stale-While-Revalidate UX for the Browse Catalog

**Axis**: Design patterns / precedent
**For**: `/hex-plan high` — browse catalog SWR (serve cached rows instantly past TTL, revalidate in background, priority-refresh the selected row)
**Context in-tree** (not re-derived here): `src/catalog.rs:11` — browse catalog cache is "atomically written, with a 1 hour TTL". Two background-check machines already exist with bounded mpsc + semaphore + generation-stamped messages + RAII in-flight dedup: `src/tui/update_check.rs`, `src/tui/bundle_member_fetch.rs`.

---

## 1. How mature package managers pace index revalidation

| Tool | TTL? | Conditional revalidation? | Serves stale while revalidating? |
|---|---|---|---|
| **Cargo** (sparse registry, RFC 2789) | **No TTL.** Every metadata fetch is a conditional request. | Yes — caches `ETag`/`Last-Modified` per index file, sends `If-None-Match`/`If-Modified-Since` on every refresh; server answers 304 or 200. ETag wins if both present. | No explicit SWR — but because revalidation is one cheap round trip (304 is near-free, HTTP/2 multiplexes it), the *effective* UX is "always check, rarely pay for it." [Cargo Book: Registry Index](https://doc.rust-lang.org/cargo/reference/registry-index.html), [RFC 2789](https://rust-lang.github.io/rfcs/2789-sparse-index.html) |
| **npm** (`_cacache`, npm ≥5) | No fixed TTL by default; behavior is a tri-state flag, not a clock. `--prefer-online` forces 304 revalidation ignoring staleness; `--prefer-offline` skips the 304 round trip and serves cache unconditionally if present; plain `--offline` errors (`ENOTCACHED`) on a miss. `cache-min`/`cache-max` (TTL-flavored) were **deprecated in npm 5.0** in favor of these two flags. | Yes, via ETag/Last-Modified stored in `_cacache/index-v5`, content addressed separately in `content-v2`. | Effectively yes under default settings — install commands read cache first and only pay for revalidation opportunistically, not on a hard clock. [npm 5.0 blog](https://blog.npmjs.org/post/161081169345/v500) |
| **apt** | **Yes, an explicit TTL** — the `Valid-Until` field in the `Release` file, default policy window commonly ~10 days for Debian/Ubuntu archives (`Acquire::Check-Valid-Until`, default `true`). Past `Valid-Until`, apt **hard-fails** (`Release file expired`) rather than serving stale. | Partial — `Release` carries `Last-Modified`/checksums per component, but the primary staleness gate is the `Valid-Until` clock, not a conditional GET per se. | **No.** This is the "TTL as a wall" anti-pattern: past expiry, `apt update` refuses to proceed until you either fetch fresh or explicitly disable the check (`-o Acquire::Check-Valid-Until=false`). [Debian apt.conf(5)](https://manpages.debian.org/unstable/apt/apt.conf.5.en.html), [reprepro `ValidFor`](https://manpages.debian.org/unstable/reprepro/reprepro.1.en.html) |
| **Homebrew** (JSON API, `formulae.brew.sh`) | **Yes, a short TTL** — `HOMEBREW_AUTO_UPDATE_SECS` gates how often `brew update` runs automatically before an auto-updating command (`install`/`upgrade`/`tap`), **default 60s** (interactive) / longer in CI. Independently, CDN-fronted API responses are commonly cached ~15 min at the edge. | Not primarily — the JSON API blobs are fetched wholesale on the auto-update cadence rather than conditionally validated per formula. | No — it's a polling TTL: cheap enough (60s) that staleness is rarely visible, but it is a clock, not a revalidation protocol. [Homebrew `EnvConfig`](https://docs.brew.sh/rubydoc/Homebrew/EnvConfig.html), [PR #14943 "Auto-update from the API less often"](https://github.com/Homebrew/brew/pull/14943) |
| **Go module proxy + checksum DB** | No TTL for the checksum database (immutable, content-addressed — a version's hash never changes once published, so nothing to revalidate). `GOPROXY` mirrors cache indefinitely and treat entries as immutable. | N/A for immutable data. `GONOSUMCHECK`/`GOPRIVATE` bypass verification for modules outside the public sumdb, not a staleness mechanism. | N/A — immutability sidesteps the whole staleness question. [go.dev: Module Mirror and Checksum DB](https://go.dev/blog/module-mirror-launch) |

**Pattern**: the tools with a hard TTL (apt, Homebrew) either fail closed (apt) or poll so cheaply it's invisible (Homebrew's 60s). The tools that dropped the TTL (cargo, npm) did so *because* they had a cheap conditional-revalidation primitive available — the TTL became redundant once every read could self-certify freshness for near-zero cost.

## 2. Is a TTL the right knob once revalidation is one round trip?

No — once conditional revalidation exists, a TTL should demote from "gate" to "network-suppression floor." Concretely:

- **Cargo and npm dropped the TTL as the primary gate.** npm's own changelog frames `cache-min`/`cache-max` (TTL semantics) as *deprecated* in favor of `prefer-online`/`prefer-offline` — a policy toggle, not a clock. Cargo never had one; every read is a conditional request, and the 304 path is cheap enough that "always revalidate" and "TTL of zero" are the same design in practice.
- **apt is the cautionary tale for the opposite choice**: `Valid-Until` as a hard wall produces the well-documented "Release file is expired" failure mode when a mirror stalls or a clock drifts — the exact failure this plan is trying to avoid (a blocking rebuild in front of the user, just triggered by clock skew instead of TTL expiry).
- **Homebrew keeps a TTL but makes it deliberately short and non-blocking** (60s) — it's a rate limiter on *how often to bother checking*, not a freshness contract. That's the "TTL as floor" pattern: prevents a request storm, never blocks a read.

**Failure modes observed**:
- *Short TTL as hard gate* (naive version of today's grimoire behavior): every expiry is a synchronous, user-visible rebuild — exactly the bug this plan fixes.
- *No TTL, revalidate always* (cargo): fine when revalidation is O(1) round trips per file: cargo's sparse index is one file per crate, so "always" doesn't mean "all N files, every time" — it means "one cheap check per file touched this session." A registry-wide catalog (many rows) is a different shape: revalidating the *whole* browse set on every open is not free the way one crate lookup is.
- *TTL as wall with no override path surfaced* (apt): user-hostile when the clock is right but the network is briefly degraded.
- *TTL as floor* (Homebrew): the safest shape for a bulk index — bounds worst-case network chatter without ever blocking a read.

**For this codebase**: keep the 1-hour TTL (`src/catalog.rs:11`) as exactly that — a floor that suppresses a full-catalog background refresh sweep more often than hourly — but stop treating TTL expiry as a reason to block the read. The row-level "user is looking at this specific thing" refresh should behave like cargo (revalidate essentially on open, since it's one row = O(1) cost), not like the bulk catalog.

## 3. Stale-while-revalidate as a formal pattern (RFC 5861)

[RFC 5861](https://www.rfc-editor.org/rfc/rfc5861) defines two `Cache-Control` extensions:

- **`stale-while-revalidate=N`**: a cache **MAY** serve the stale response for up to `N` seconds past expiry while it revalidates in the background — the client that triggers the revalidation still gets the fast stale answer; the *next* request after revalidation completes gets the fresh one. This is explicitly a latency-hiding mechanism: "hiding [network and server] latency from clients." It bounds *how stale is too stale* with that same `N` — past `N` seconds stale, the cache must go synchronous (block and fetch) again, same as if the extension weren't present.
- **`stale-if-error=N`**: separately, permits serving stale on an *origin error* (5xx, timeout, DNS failure) for up to `N` seconds, decoupled from the revalidate case — this is the "grace window that turns an outage into a non-event" pattern.

Two things the RFC prescribes that matter for this plan:
1. **The staleness bound is explicit and finite**, not "forever." A `stale-while-revalidate` window without an upper bound isn't the pattern — it's just "never expire."
2. **Revalidation is triggered by *the read that finds it stale*, off the request path** — the caller that discovers staleness kicks off the background fetch but does not wait on it. This maps directly to "serve cached rows instantly, revalidate in the background" as specified in the task — that *is* RFC 5861's shape, just applied to an in-process catalog cache instead of an HTTP intermediary.

## 4. Interactive TUI precedent for "refresh what the user is looking at"

| Tool | Mechanism | Concrete interval | Source |
|---|---|---|---|
| **lazygit** | Two independent polling clocks, not selection-driven: `refresher.refreshInterval` (local file/submodule state) and `refresher.fetchInterval` (remote ref polling), each independently disable-able via `git.autoRefresh`/`git.autoFetch`. | **10s** file refresh, **60s** remote fetch (defaults). Docs explicitly warn that a 1s `fetchInterval` causes UI-blocking pauses. | [lazygit Config.md](https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md) |
| **k9s** | Flat poll loop against the Kubernetes API, not selection-scoped — the whole visible resource list refreshes on the same clock. | `refreshRate`, **default 2s**; docs recommend raising to 5–10s on large clusters to reduce API server load. | [k9s config docs](https://k9scli.io/topics/config/) |
| **btop** | System-monitor poll loop, `update_ms`, **default 2000ms**, tunable down to a hard floor of **100ms** (documented reason: below that, both data accuracy and input responsiveness degrade). | 2000ms default, 100ms floor | [btop manpage](https://github.com/aristocratos/btop/blob/main/manpage.md) |
| **yazi** | The one tool here with genuine *selection-driven* (not clock-driven) refresh: file preview generation is tied to cursor movement, not a timer. Uses `CancellationToken` for previewers and `JoinHandle::abort()` for in-flight async folder loads — moving the cursor **cancels** the in-flight preview task for the row you left, rather than letting it land and discarding. | No published fixed debounce number; cancellation is immediate on selection change, not time-gated. | [yazi preview management (DeepWiki)](https://deepwiki.com/sxyazi/yazi/6.1-keybindings), [yazi #2144 task cancellation](https://github.com/sxyazi/yazi/issues/2144) |
| **gitui** | Uses filesystem-watch–triggered invalidation (notify-driven, not polling) for the working tree; no publicly documented fixed debounce constant was found for its refresh path in this pass — treat as directional precedent only, not a cited number. | not found | — |

**Two distinct precedent families**:
- **Flat-clock tools** (lazygit, k9s, btop) refresh the *whole visible list* on a fixed interval, independent of cursor position — 2–10s is the observed band, with explicit documentation that sub-second intervals cause UI jank or resource pressure.
- **Selection-driven tools** (yazi) refresh only *the item under the cursor*, cancel-on-move, and have no fixed debounce — cancellation is what prevents the request storm, not a timer.

Neither family uses a debounce *timer* keyed to "selection settled" in the way this plan's row-refresh does — that shape (wait N ms after the cursor stops moving, then fetch just that row) is closer to the search-typeahead debounce pattern (below) than to any of these TUIs' native refresh loop.

## 5. Known pitfalls of background revalidation in a TUI

1. **Request storm on fast scroll.** If a fetch fires per row visited, holding an arrow key generates one request per row passed over. yazi avoids this by cancelling the previous row's task immediately on move (no request survives past its row's tenure) rather than debouncing; a debounce-based design avoids it by not firing until the cursor is idle for N ms.
2. **Late results landing after the user moved on.** If in-flight work isn't cancelled, a slow response for row A can arrive and repaint *after* the user has moved to row C — this is the exact bug class the existing generation-stamped message pattern (`src/tui/update_check.rs`) already guards against for the two shipped background-check machines: a response tagged with a stale generation is dropped, not rendered. The row-refresh feature should reuse that same generation check, not invent a second discard mechanism.
3. **Flicker under the cursor.** Repainting a row's fields the instant fresher data arrives — while the user's eye is still on it — reads as instability even when the data is more correct. RFC 5861's model implies the fix: swap only when the *next* interaction (not the in-flight one) observes the fresh value, or at minimum, animate/diff the change rather than a hard repaint.
4. **Losing cursor position on redraw.** A background refresh that re-sorts or re-filters the list (e.g., a newly-visible row shifts sort order) can move the cursor out from under the user. Any redraw triggered by a background revalidation must preserve the user's *selected identity* (row key), not their *row index* — reselect by key after the redraw, don't assume the index is still valid.

## 6. Recommendation

**Keep the 1-hour TTL, but demote it from gate to floor** — it now governs only "how often to sweep the *whole* catalog in the background," never "may I answer this read." Every open of the browse view serves whatever is cached immediately, unconditionally, then kicks a background revalidation if the TTL has lapsed (Homebrew's model: a clock that suppresses redundant full-catalog network chatter, not a wall that blocks a read — contrast apt's `Valid-Until`, which is the failure mode being fixed).

**Row-level refresh** (the row the user has selected):
- **Debounce**: **250–300ms** after the selection stops moving before firing the row's individual metadata fetch. Justification: this is the well-established typeahead/autocomplete debounce band (200–500ms, converging on ~300ms as "feels instant, gives the system breathing room") — a TUI arrow-key selection walk is the same input shape as keystrokes in a search box (bursty, directional, self-interrupting), so the same empirical band applies. None of the surveyed TUIs (lazygit/k9s/btop) debounce selection at all — they poll the whole list on a flat clock — so there is no closer domain precedent than typeahead; yazi is the only selection-driven precedent and publishes no fixed number.
- **Cancel-on-move, don't let-it-land-and-discard**: follow yazi's model, not a discard-by-generation model, *for the row fetch specifically*. Reasoning: discard-on-arrival (what `update_check.rs` already does for the bulk background sweep) is right for background work the user isn't watching — wasted work is invisible. But the row-level fetch is triggered by active attention on that row; letting a stale request run to completion after the user has moved away burns a network round trip and a semaphore slot for an answer nobody will see, when the semaphore is a shared, bounded resource per the existing machinery. Cancel it. In Rust with Tokio this is `JoinHandle::abort()` or a `CancellationToken`, matching yazi's implementation exactly — RAII in-flight dedup already in the codebase (`src/tui/bundle_member_fetch.rs`) is the natural place to add a cancellation hook, since it already tracks one in-flight fetch per key.
- **Reuse the existing generation stamp for the *bulk* SWR sweep**, not the row fetch: the background full-catalog revalidation triggered by TTL lapse should tag its results with a generation the way `update_check.rs` already does, so a result that lands after the user has navigated away from the browse view entirely is dropped rather than repainting a screen nobody's looking at.
- **Bound staleness explicitly, RFC-5861-style**: don't serve arbitrarily old cache forever just because nothing has triggered a revalidation. If the cache is older than some outer bound (e.g., a small multiple of the TTL — RFC 5861 callers commonly set `stale-while-revalidate` to the same order of magnitude as `max-age`, not 10-100x it), treat that as "no usable cache" and fall back to today's blocking fetch. This preserves a hard ceiling on staleness while making the common case (published 3 minutes ago, TTL is 1 hour) always hit the fast instant-serve path.
- **Preserve selection identity across any redraw** triggered by a landing background result — reselect by row key, not index, per pitfall 5.4 above.

### Top 3 findings
1. **The tools that dropped a TTL (cargo, npm) did so because they gained cheap conditional revalidation, not because TTLs are bad** — apt keeps a hard TTL and fails closed on it (the exact anti-pattern grimoire has today); Homebrew keeps a TTL but makes it a cheap, non-blocking floor. Grimoire's fix should follow Homebrew's shape for the bulk sweep: TTL stays, but only gates "should I refresh in the background," never "may I answer now."
2. **RFC 5861's `stale-while-revalidate` is exactly the requested design already**, and it prescribes an explicit, finite staleness bound (the same order of magnitude as the base TTL) — not "serve forever until something happens to trigger a refresh."
3. **No mainstream TUI debounces selection-driven refresh on a timer** — flat-clock tools (lazygit 10s/60s, k9s 2s, btop 2000ms) poll everything regardless of cursor; the one selection-driven tool (yazi) cancels on move instead of debouncing. The closest sourced precedent for "wait for the cursor to settle before fetching" is the general UI typeahead-debounce band (~300ms), not a TUI-specific number.

**Sources**:
- [Cargo Book — Registry Index](https://doc.rust-lang.org/cargo/reference/registry-index.html)
- [RFC 2789 — Sparse Index](https://rust-lang.github.io/rfcs/2789-sparse-index.html)
- [npm 5.0.0 blog — prefer-online/prefer-offline](https://blog.npmjs.org/post/161081169345/v500)
- [Debian apt.conf(5) — Acquire::Check-Valid-Until](https://manpages.debian.org/unstable/apt/apt.conf.5.en.html)
- [reprepro(1) — ValidFor](https://manpages.debian.org/unstable/reprepro/reprepro.1.en.html)
- [Homebrew::EnvConfig — HOMEBREW_AUTO_UPDATE_SECS](https://docs.brew.sh/rubydoc/Homebrew/EnvConfig.html)
- [Homebrew PR #14943 — auto-update from the API less often](https://github.com/Homebrew/brew/pull/14943)
- [go.dev — Module Mirror and Checksum Database Launched](https://go.dev/blog/module-mirror-launch)
- [RFC 5861 — HTTP Cache-Control Extensions for Stale Content](https://www.rfc-editor.org/rfc/rfc5861)
- [lazygit Config.md](https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md)
- [k9s config docs](https://k9scli.io/topics/config/)
- [btop manpage](https://github.com/aristocratos/btop/blob/main/manpage.md)
- [yazi preview management — DeepWiki](https://deepwiki.com/sxyazi/yazi/6.1-keybindings)
- [yazi issue #2144 — task cancellation](https://github.com/sxyazi/yazi/issues/2144)
- Typeahead debounce band (~300ms): [spin.atomicobject.com — Autocomplete Timing](https://spin.atomicobject.com/2018/06/04/automplete-timing-debouncing/), [Algolia — Debounce sources](https://www.algolia.com/doc/ui-libraries/autocomplete/guides/debouncing-sources)
