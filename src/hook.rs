// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Hook consent and hook forensics: the two things that stand between a
//! resolved `hook` artifact and code a client runs automatically.
//!
//! Four submodules, four questions:
//!
//! - [`consent`] — *may hooks declared by this checkout arm?* The
//!   machine-local, workspace-keyed consent record and the pure predicate
//!   over it. This is **T3**'s control: the user is not vouching for a
//!   repository by cloning it.
//! - [`trust`] — *given consent, does this hook arm on this invocation?*
//!   The composition of the feature flag, the `--trust-hooks` pair, the
//!   plain-HTTP transport gate and [`consent`]'s answer, plus the
//!   one-time prompt and its non-interactive contract (C-023).
//! - [`policy`] — *what did this invocation decide?* The owned,
//!   clonable resolution of [`trust`]'s inputs that travels down to the
//!   convergence seam while the prompt stays at the command boundary.
//! - [`audit`] — *what happened when it ran?* The redacted metadata
//!   record (C-012), its sanitization, size cap and rotation.
//!
//! Deliberately **not** here, and deliberately not reachable from here:
//!
//! - The **approval store.** There is none, and [`consent`] is not one.
//!   Consent is recorded per *workspace*, never per hook: no per-artifact
//!   record, no digest key, no hash chain, no `hook_approvals.json`
//!   (owner decision 2026-08-14, ADR amendment A2 reversing D5; the ADR
//!   carries two `WITHDRAWN` banners at decision E and contract C-009
//!   precisely because a reader working top-to-bottom would rebuild it).
//!   A2's coarseness is kept exactly; what [`consent`] restores is the
//!   *directory* half of decision E's key, which A2 deleted as collateral
//!   — see that module's doc.
//! - **Any environment variable.** Neither `GRIM_EXPERIMENTAL_HOOKS` nor
//!   `GRIM_ALLOW_HOOKS` exists (owner decision 2026-08-17, landed in
//!   `24a14bb`, withdrawing C-026). The feature flag is
//!   `options.experimental.hooks`, set through `grim config set`; consent is
//!   the workspace's own machine-local record, written by `grim hook allow`
//!   and never by a config file; `--trust-hooks` is a per-invocation CLI
//!   flag. Three questions, three places, and **no environment-vs-config
//!   precedence rule left to get wrong.** There is likewise no
//!   `GRIM_HOOK_CONSENT` and no env form of the record path, for the
//!   identical CWE-426 reason.
//! - **[`crate::env::grim_home`].** Nothing in this module may call it.
//!   It returns its environment value verbatim — no absoluteness check,
//!   and a *relative* `.grimoire` fallback when `HOME` is unset — so a
//!   runtime path derived from it is chosen by whoever controls the
//!   client's environment (audit finding B1, T3, CWE-426). Every path
//!   these modules touch arrives as a parameter, resolved and absolute,
//!   from a caller that established it at install time.
//!
//! ## Two invariants shape every signature below
//!
//! **I3 — grim fails in the direction that does not block the user.**
//! An internal error, a missing file, an unparsable record or an unknown
//! schema version degrades to *"the feature is off"*, never to *"the
//! agent is blocked"*. This is a real availability obligation, not
//! politeness: Copilot's `preToolUse` is **fail-closed**, so any non-zero
//! exit grim produces denies the user's tool call. Where a contract here
//! says "fail closed", it means *do not spawn the payload and exit 0* —
//! never *emit a deny verdict*. See [`audit`]'s module doc, where that
//! distinction is the whole subject.
//!
//! **I5 — tamper-evidence, not tamper-resistance.** Under a machine
//! compromised at grim's own privilege (N2, an explicit non-goal) nothing
//! here prevents an edit; a same-privilege process can rewrite config,
//! the audit log, and grim's own binary. What these modules ensure is
//! that no edit *arms* anything without a subsequent grim command whose
//! inputs are version-controlled and visible in `git diff` / `grim
//! status`. Never describe a control here as prevention when it is
//! evidence.

pub mod audit;
pub mod consent;
pub mod policy;
pub mod trust;
