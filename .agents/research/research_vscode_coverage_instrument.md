# The VS Code extension's coverage number is not test coverage

Recorded 2026-08-19 during the ratings deferred-findings run (F-8, WP DF-H).
Repo: `grimoire-vscode`, branch `feat/artifact-ratings`.

## What happened

F-8 was written against `src/views/vote.ts` reporting **39.48% lines, 0%
functions**. Adding six unit tests that demonstrably execute the file moved the
number by exactly zero. The number was not wrong — it was answering a different
question than the one it was read as answering.

## Why

`.vscode-test.mjs` instruments one file:

```js
coverage: { includeAll: true, include: ['dist/extension.js'], exclude: [...] }
```

`esbuild.tests.js` bundles **each** `src/test/*.test.ts` separately into
`out/test/`. So `src/views/vote.ts` is compiled into two independent bundles,
each with its own inlined copy. A unit test importing `../views/vote` gets the
`out/test/` copy — a different module object, outside the instrument.

Proven, not inferred. A `console.error(\`…${__filename}\`)` probe in each of
`vote.ts`'s functions, one full-suite run:

```
12 castVote        …/out/test/rating.test.js
 1 confirmVote     …/out/test/rating.test.js
10 refineVoteState …/out/test/rating.test.js
    (dist/extension.js: zero)
```

Pointing a throwaway config at the other bundle does not help: the report came
back with **149 `SF:` entries — exactly `dist/extension.js.map`'s source count,
zero `src/test/*`**. The harness only ever collects the extension bundle.

Confirmed from the other direction: adding one *extension-level* test that
drives the vote through the activated extension moved `vote.ts` from
`39.48 / 100 / 0` to `62.56 / 30 / 50` — the first real branch number that file
has had.

## Reading the column

`X | 100 | 0` (branch 100%, funcs 0%) is the tell. It is not "well branched,
badly functioned" — it is `0/0` branches rendering as 100% beside `0/N`
functions. **Any file showing it was never entered by the instrumented bundle.**

Same run, same signature: `grimInfo.ts` 31.94/0%, `installer.ts` 53.39/27.77%.
The number rewards integration tests and is blind to unit tests — the inverse of
what this codebase is organised around, since `CLAUDE.md` asks for pure modules
"no vscode, no DOM imports, so they stay unit-testable".

## Before changing `include`, decide what the number is for

- **"Is this logic tested?"** — it has to see the test bundles. Not reachable
  through `@vscode/test-cli`'s coverage as configured; needs investigation.
- **"How much of the shipped extension does the suite exercise?"** — it is
  already correct, and the fix is documentation: stop reading per-file rows as
  test coverage.

Do not "fix" it by widening `include` without picking one. A merged number over
both bundles would double-count every shared source file and be less legible
than what is there now.

## Why this is worth recording

F-8 existed because a number nobody could interpret was read as a coverage gap.
The finding closed on tests, not on the percentage. The next person reading a
low row on a pure module will reach the same wrong conclusion unless the column
is documented or fixed.
