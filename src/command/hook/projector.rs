// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Tests that pin the projection table against grim's vendor matrix; the projector itself lives in
//! `grimoire-hook-core`.

#[cfg(test)]
mod tests {
    use grimoire_hook_core::{CanonicalEvent, HookTier, projection_for};

    use crate::install::client_target::ClientTarget;
    use crate::install::vendor::KindSupport;

    /// **C-021 — `hook_tier_support` is a QUERY over `RESPONSE_PROJECTION`,
    /// never a second copy of it.**
    ///
    /// The expectation is recomputed here from the table's three rules, for
    /// **every** client in `ClientTarget::ALL` and every `(tier, event)` pair.
    /// That is what makes a re-spelling detectable: a hand-written `match` that
    /// agrees with the table today stops agreeing the moment a row moves, and
    /// this test — which reads the row — is what fails. A test comparing against
    /// literals could not tell the two implementations apart at all.
    ///
    /// The rules, verbatim from `Vendor::hook_tier_support`'s own doc: no
    /// surface or no row ⇒ `Declined`; the tier's required field absent ⇒
    /// `Declined` (`gatekeeper` → a non-empty `verdict`, `mutator` →
    /// `mutation`, `observer` → nothing); an absent `context` ⇒ `Degraded`;
    /// else `Native`.
    #[test]
    fn hook_tier_support_is_a_query_over_the_projection_table_c021() {
        for client in ClientTarget::ALL {
            let vendor = client.vendor();
            for event in CanonicalEvent::ALL {
                let row = projection_for(vendor.name(), event);
                for tier in HookTier::ALL {
                    let expected = if vendor.hook_surface().is_none() {
                        KindSupport::Declined
                    } else {
                        match row {
                            None => KindSupport::Declined,
                            Some(row) => {
                                let required_present = match tier {
                                    HookTier::Gatekeeper => !row.verdict.is_empty(),
                                    HookTier::Mutator => row.mutation.is_some(),
                                    HookTier::Observer => true,
                                };
                                if !required_present {
                                    KindSupport::Declined
                                } else if row.context.is_none() {
                                    KindSupport::Degraded
                                } else {
                                    KindSupport::Native
                                }
                            }
                        }
                    };
                    assert_eq!(
                        vendor.hook_tier_support(tier, event),
                        expected,
                        "{client}: hook_tier_support({tier}, {event}) disagrees with the one \
                         projection table — a second copy of the matrix has appeared, and its \
                         drift direction is 'the runtime emits a field render-time forbade'"
                    );
                }
            }
        }
    }
}
