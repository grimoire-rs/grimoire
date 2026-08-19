# Round 3 — fix verification, pass 2 (V1 / V2 / V3)

Verification of the fixes for [`round3-fix-verify.md`](./round3-fix-verify.md)'s
V1, V2 and V3, at tip **`d7da56f`** (`feat(hook)`, holds all `src/`), tree clean.
Scope is those three findings only.

Baselines on this tree:

```
cargo test --quiet                                  → 2947 passed; 0 failed
test/ pytest (9 hook + docs + rig files)            → 132 passed
```

## Verdict

| Finding | Fix | Verdict |
|---|---|---|
| **V1** envelope name representable in the binding namespace | separators → underscores (`payload_<pid>_<slot>.json`) | **Substance fixed and the class is closed** (executed enumeration) — but **the fix has no test that observes it** → **W1**, and one claim about it is false |
| **V2** the drift test's promise exceeded its reach | scoping sentences added to `oci::hook` and hook-spec.md | **The added sentences are true**; the false sentence they scope was **left in place** → **W2** |
| **V3** stale count + wrong error message in hook-spec.md | count dropped; row replaced with the binary's output | **Closed**, both halves verified programmatically |

Mutation matrix (scratch copy of `d7da56f`, baseline 2947 green):

| Mutant | Expected | Result |
|---|---|---|
| **M18** — tidy the envelope separators back to hyphens (V1 reverted) | red, per the brief | **green, 2947 passed** → W1 |
| M19 — rename `AUDIT_FILE` to the representable `hook-audit.jsonl` | red | **red** ×2: `every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding`, `the_audit_trail_is_the_dispatch_tables_sibling` |
| M20 — drop `dispatch.json.lock` from the array (harness control) | red | **red** ×2: `every_grim_owned_name_under_hooks_is_a_reserved_binding_name`, `read_manifest_refuses_a_traversing_record_name` |
| control | green | green (2947) |

---

## Ask 1 — is V1's class dissolved, or only this one writer?

**Dissolved in fact.** Re-derived twice, from the code and from a running
machine, without consulting your list.

*Static* — every site that writes at the root of `hooks/`: `dispatch_path`
(`atomic_write`, which also drops a `NamedTempFile::new_in` sibling),
`root_key_path` (`create_new`, no temp), the table's `AdvisoryFileLock` sidecar
(the only advisory lock anywhere under `hooks/` — grepped), `hook_launcher`
(`bin/`), `payload_relative` (`payload/`), `AuditLog` (`OpenOptions::append` on
`hook_audit.jsonl`, `fs::rename` to `…​.jsonl` + `ROTATED_SUFFIX`), and
`write_payload_file` (`private_dir` = `invocation.audit.path().parent()`).

*Dynamic* — one lifecycle (global install of a `payload = "file"` observer +
project install + two firings + a forced rotation past the 8 MiB
`MAX_LOG_BYTES`), listing the root both from the outside and from inside the
payload while its envelope existed, then asking **production code** for the
verdict on each observed name (`grim add --global … --name <observed> --kind hook`):

| Observed | Verdict from `grim add` |
|---|---|
| `bin` | refused — reserved |
| `dispatch.json` | refused — reserved |
| `payload` | refused — reserved |
| `root-key` | refused — reserved |
| `hook_audit.jsonl` | refused — grammar (`must contain only lowercase letters, digits, hyphens, and …`) |
| `hook_audit.jsonl.1` | refused — grammar |
| `payload_2089182_0.json`, `payload_2089189_0.json` | **refused — grammar** ← the V1 fix, executed |
| `file-probe` | accepted — the artifact's own payload directory, by design |

`dispatch.json.lock` did not appear in the post-hoc listing (the guard releases
it before `converge_root` returns) and is reserved anyway, pinned by M20.
`atomic_write`'s `.tmp*` sibling is unrepresentable by the leading dot — I did
not race a listing against it, so that one row rests on the code plus tempfile's
default prefix, not on an observation.

So **no representable, unreserved name is written at that root any more**, and
the underscore fix is not local in effect: the audit trail was already safe by
the same rule and the rotation suffix inherits it. `every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding`
is a good addition — it closes the axis the filesystem walk cannot reach, and M19
shows its audit rows really are derived from the constants that produce them.

### W1 (Warn) — the V1 fix is unguarded, and two claims say it is guarded

**M18: change `write_payload_file`'s format string back to
`payload-{pid}-{slot}.json` and the whole suite stays green — 2947 passed.**

Both intended pins construct the name themselves rather than reading it from the
code that produces it:

* `pipeline.rs:1691` — `let name = format!("payload_{}_{}.json", 12345, 0);`
* `hook_dispatch.rs:1071` — `let envelope = format!("payload_{}_{}.json", 4294967295u32, 0);`

There is no constant or helper for this name — the format string is inline at
`pipeline.rs:1092` — so neither test can observe a change to it. No acceptance
test covers it either (grepped `test/` for the name shapes and for
`GRIM_HOOK_PAYLOAD`).

Two false claims follow from that:

1. Your brief: *“New test `the_payload_envelope_name_cannot_be_a_binding_name`,
   proven red when the separator is tidied back to a hyphen.”* It is not — M18.
   (`d7da56f`'s commit message does **not** repeat this claim; it says only that
   the separators are underscores now, which is true.)
2. `hook_dispatch.rs:1059-1060`, in the class-level test's own doc: *“every row
   reads its name from the constant that produces it instead of copying it.”*
   True of the `audit` and `rotated` rows (M19 proves it). **False of the
   envelope row** — it copies a format string — and of the `.tmp` row, which is
   the hardcoded literal `".tmpAbC123"` rather than anything tempfile produces.

This is the same shape as W5 last round: a test that re-spells the thing it
claims to pin, in the test written to close the finding. The comment in
`write_payload_file` ends *“Do not ‘tidy’ these to hyphens”* — a comment is the
guard you replaced underscores for precisely because a comment is not a guard.

Fix, cheap and mechanical: give the name one home —

```rust
fn envelope_file_name(pid: u32, slot: u64) -> String { format!("payload_{pid}_{slot}.json") }
```

— call it from `write_payload_file` and from both tests. M18 then fails. Correct
the `.tmp` row's claim at the same time (it is a hand-written literal standing in
for tempfile's default prefix; say so, or drop the row's half of the sentence).

Everything else about V1 is right, and the reasoning is better than either option
I offered: unrepresentable beats unguessable, reserving dynamic names was
impossible, and the corrected `EXPECTED_UNRESERVED` note now states the location
plainly (*“Both sit at the root of `hooks/` — an earlier version of this note said
the envelopes lived inside a payload directory, which was false”*) with the
grammar as the reason.

---

## Ask 2 — are the V2 scoping sentences true?

The sentences you **added** are true, and I re-proved the property they describe:
M18 is the same demonstration as the original M10 — a change to a runtime-side
writer's name leaves the suite green, so the walk's reach really does stop at
install's dispatch-side writes.

### W2 (Warn) — the false sentence was scoped, not removed, so the paragraph now asserts and retracts the same claim

`src/oci/hook.rs:186-188` still reads:

> … then requires **every directory entry it finds** to be refused as a binding
> name. **So it fails for the next file grim puts there whether or not anyone
> remembers to tell it about the file.**

Twelve lines below, `:196-203`:

> **Its reach is install's dispatch-side writes, and no wider.** … An earlier
> version of this paragraph claimed the test fails *“whether or not anyone
> remembers to tell it about the file”*, which was true only on the axis it
> covers.

That sentence is not in an earlier version — it is 12 lines up, in this one. A
reader who stops at the guarantee (the natural stopping point: it closes the
paragraph that explains the mechanism) gets the false promise, and the
correction reads as being about text that is no longer there. Scope it in place
— *“for the next file **install** writes there”* — and let the paragraph below
explain the limit.

`hook-spec.md:341-346` has the same pattern in weaker form: *“so it catches a new
file whether or not anyone tells it about one”* immediately followed by *“**Its
reach stops at install's dispatch-side writes**”*. A reader gets the truth from
the pair, but the first clause should carry the qualifier rather than be walked
back.

One omission in the same passage (Suggest): *“so for those the rule above is all
there is”* is no longer accurate — `every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding`
covers the runtime-side names by list, and M19 shows it bites. It will not catch a
**new** runtime writer, which is presumably what you meant; say that, and name the
test as the place to add the row, so the next author lands in it.

---

## Ask 3 — remaining false claims

Beyond W1's two and W2's one:

### W3 (Warn) — `.claude/rules/subsystem-file-structure.md:197` states a count the table below it contradicts

> Grim writes several things directly under **`$GRIM_HOME/hooks/`** … **The three
> below** are the ones with their own semantics; the namespace also holds
> `dispatch.json.lock` (the table's advisory lock sidecar), …

The table under that sentence has **four** rows — `dispatch.json`,
`bin/grim-hook`, `root-key`, and `dispatch.json.lock`, which the same edit added
both to the prose's “also holds” list *and* to the table. This is the V3 defect
(a stale count beside the list it counts) reappearing in the file the V4 edit
rewrote, in an always-loaded rule. Drop the count, as you did in hook-spec.md,
and pick one home for the sidecar.

Everything else I checked held:

* `d7da56f`'s commit message. Its round-3 section is accurate, including the
  three fix-verify items; notably it does **not** claim the envelope name is
  test-pinned, which is the claim that fails in your brief.
* The V1 doc paragraphs in `oci::hook` and `EXPECTED_UNRESERVED`: both now say
  the envelopes sit at the root of `hooks/` and give the grammar as the reason —
  matching `write_payload_file`'s own doc and the rule file, so the three no
  longer disagree.
* hook-spec.md's authoring rule now states the underscore route explicitly
  (*“unless the name is unrepresentable as a binding name, which an underscore
  achieves”*) and names both families that rely on it.
* `write_payload_file`'s historical paragraph still describes the old
  `payload-<pid>-<artifact>-` prefix — correctly, as history (finding P-6's
  narrative), not as the current format.

## V3 — closed, verified programmatically

* No count remains: *“The reserved check is exact string equality against **that
  list**”*, and the five-name sentence is the only enumeration.
* The pitfalls row is the binary's own output. Executed on this build and
  compared as strings rather than by eye:

  ```
  binary says: hook artifact name 'bin' is reserved: 'bin' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact
  appears in hook-spec.md verbatim: True
  ```

  (`grim build` exits 65, as the doc block beside the row states.) Nothing
  automated ties the two together, so this is a byte-for-byte agreement that
  holds today rather than a pinned one — acceptable for a docs row, worth knowing
  if the message is ever reworded.

---

## Addendum — the re-fold (`18193c6`) and the new class-level test

Everything above was produced against the working tree that `18193c6` folded, not
against `d7da56f`: the scratch copy I mutated already contained
`every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding` (M19's
failure names it), and its baseline was 2947 — the count you report. `git diff
d7da56f 18193c6 -- src/` is exactly that test plus `AUDIT_FILE` becoming `pub`,
and neither touches the envelope's format string, so **W1 stands unchanged**. I
re-ran M18 against a fresh copy of the current tree to remove any doubt:
**green, 2947 passed**.

So the claim in the brief — *“Every row reads its name from the constant that
produces it rather than copying the literal. Proven red by putting the envelope's
hyphens back.”* — is false on both halves for the envelope row, and that row is
the one V1 is about. Production is still an inline `format!` at
`pipeline.rs:1092`; the row is an independent `format!` at
`hook_dispatch.rs:1071`, as is `pipeline.rs:1691`. There is no constant for it to
read.

### Are the four rows the complete set of runtime-side writers?

**Yes for the runtime, and one row is filed under the wrong writer.** Every
write site in the runtime path, grepped across `src/command/hook/**` and
`src/hook/audit.rs` (production lines only):

| Site | What it writes at the root of `hooks/` |
|---|---|
| `pipeline.rs:1093` (+ `:912` removal) | the envelope, `payload_<pid>_<slot>.json` |
| `audit.rs:549`, `:556` | `create_dir_all(parent)` — no new name — then opens the trail |
| `audit.rs:581` | `fs::rename` to `<trail>` + `ROTATED_SUFFIX` |

That is the whole set: the runtime takes no lock, never calls `atomic_write`, and
creates no other file (grepped for `atomic_write`/`AdvisoryFileLock` under
`src/command/hook/` — no hits). So rows 1–3 are complete.

Row 4 is not a runtime writer. `.tmp*` siblings under `hooks/` come from
`converge_root`'s `atomic_write`, which is **install**-side; the runtime never
calls it. The row is harmless and its reason (leading dot) is right, but the
doc introduces the list as “each runtime-side writer's format”, and a
hand-maintained list whose entries are attributed to the wrong subsystem is
exactly how the next person mis-scopes it. Either relabel the list “every writer
the directory walk cannot observe” — which is the honest common property, since
the temp sibling is gone before the walk lists — or move that row's reason next
to `atomic_write`.

### Do the `AUDIT_FILE` / `ROTATED_SUFFIX` rows genuinely drift-proof those two names?

**Yes — but not on their own; it takes the row plus a sibling test, and the
envelope has neither leg.** Three mutations, on the tree of `18193c6`:

| Mutation | Result |
|---|---|
| M19 — rename `AUDIT_FILE` to the representable `hook-audit.jsonl` | **red** ×2: `every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding` **and** `the_audit_trail_is_the_dispatch_tables_sibling` |
| M21 — writer diverges: `audit_trail_path` joins a literal `"audit-trail.jsonl"`, constant left defined and untouched | **red**: `the_audit_trail_is_the_dispatch_tables_sibling` (the class row passes — it is still reading the constant) |
| M22 — rotation diverges: `rotated.push(".previous")` instead of `ROTATED_SUFFIX` | **red**: `hook::audit::tests::the_rotated_name_appends_rather_than_replacing_the_extension` |

Reading the constant buys you the **rename** axis. It does not buy the
**divergence** axis — a writer that stops using the constant leaves the row
asserting a name nothing produces, and the row would pass. What closes that axis
for these two names is a second test that pins the derivation itself
(`the_audit_trail_is_the_dispatch_tables_sibling`,
`the_rotated_name_appends_rather_than_replacing_the_extension`), and both happen
to exist. So the two rows are drift-proof in effect, by the pair rather than by
the row.

The envelope is the one name with **neither** leg: no constant for a row to read,
and no test pinning where its name comes from. That is the same gap from two
directions, and one change closes both — extract `envelope_file_name(pid, slot)`,
call it from `write_payload_file` and from both rows, and M18 goes red while the
row stops copying a literal. Until then the class-level test's claim to have
“closed the remaining axis” holds only for names that never change, which is the
opposite of what a drift guard is for.
