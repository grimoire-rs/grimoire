# Goal: "One use-case hooks usage guide; explanation moves to reference"

Source: owner feedback on PR #176, 2026-10-01 · Written: 2026-10-01

## Definition of done

- [ ] Exactly one hooks guide (how-to) under Guides, user- and use-case based, with an authoring section whose example permits a specific action.
- [ ] The explanatory text (gating, consent, flag-off, remove, client limits, not-armed causes) lives on reference pages under Reference; existing anchors keep resolving.
- [ ] The handler contract (stdin envelope in, verdict out, per tier) is documented on a reference page, checked against `src/command/hook/`.
- [ ] Guide examples are backed by a test bound by declared key (DOC-EX-01); docs-quality checks, `task docs:check`, `task --force verify` pass.
- [ ] Branch `hex/hooks-pr-98-revival` finalized with /hex-finalize, pushed with lease, CI green on the new head.

## Autonomy

- Prompting: never — the owner said "do not prompt me by any means".
- Granted acts: /hex-finalize of `hex/hooks-pr-98-revival` including its force-with-lease push (owner, 2026-10-01).
- Forbidden: merge, pushes to main, closing PRs.
