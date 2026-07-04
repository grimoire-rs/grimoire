// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Embedded CA roots for grim's HTTPS clients.
//!
//! grim ships as a self-contained binary that must still work where the host
//! has no system trust store — a distroless or minimal CI image without
//! `ca-certificates`, or an `SSL_CERT_FILE` bundle that yields no certs.
//!
//! `reqwest` 0.13's rustls backend delegates trust to
//! `rustls-platform-verifier`. With no explicit roots it calls
//! `Verifier::new`, which loads roots *only* from the system store and
//! hard-errors when that store is empty. `oci-client`'s `Client::new` catches
//! that error and falls back to `reqwest::Client::default()`, whose internal
//! `.expect()` re-triggers the identical failure as a process-crashing panic.
//!
//! Seeding the compiled-in Mozilla root set (`webpki-root-certs`) as *extra*
//! roots takes the `Verifier::new_with_extra_roots` path instead: it never
//! errors on an empty system store and still merges whatever the platform
//! store provides. A corporate root supplied via `SSL_CERT_FILE` /
//! `SSL_CERT_DIR` keeps working *alongside* the public roots — merged, never
//! replaced (replacing is `tls_certs_only`, which grim never uses).
//!
//! Every grim HTTPS client flows through one of the two seams below so all
//! three (the `oci-client` registry client and the two direct `reqwest`
//! clients for the package index and forge APIs) share identical trust
//! anchors and the same no-panic, merge-with-system behavior.

use oci_client::client::{Certificate, CertificateEncoding};

/// The compiled-in Mozilla CA roots as `oci-client` certificates, for the
/// [`ClientConfig::extra_root_certificates`] seam.
///
/// `oci-client` feeds these to `reqwest`'s `tls_certs_merge` — the same merge
/// path [`merge_embedded_roots`] uses for the direct `reqwest` clients.
///
/// [`ClientConfig::extra_root_certificates`]: oci_client::client::ClientConfig::extra_root_certificates
pub fn oci_extra_roots() -> Vec<Certificate> {
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|der| Certificate {
            encoding: CertificateEncoding::Der,
            data: der.as_ref().to_vec(),
        })
        .collect()
}

/// Merge the compiled-in Mozilla CA roots into a `reqwest` client builder as
/// extra roots.
///
/// Uses `tls_certs_merge`, which keeps the platform verifier and *adds* these
/// roots — it does not disable the system store (that is `tls_certs_only`). So
/// the resulting client trusts the system roots, an `SSL_CERT_FILE` /
/// `SSL_CERT_DIR` override, and the embedded public roots together.
pub fn merge_embedded_roots(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // These roots are compile-time-constant, valid DER embedded in the binary,
    // so `from_der` cannot fail for them. `filter_map` keeps the builder
    // infallible rather than plumbing a `Result` through every client
    // constructor; a hypothetical parse failure drops one public root while the
    // system store and remaining roots stay intact — never a hard failure.
    let certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|der| reqwest::Certificate::from_der(der.as_ref()).ok());
    builder.tls_certs_merge(certs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the embedded root set must be non-empty. Were it empty,
    /// `reqwest`'s rustls setup takes the `root_certs.is_empty()` branch
    /// (`Verifier::new`), which panics through `oci-client`'s `Client::new`
    /// fallback on a host with no system trust store. A non-empty set forces
    /// the `new_with_extra_roots` (merge-with-system, no-panic) path.
    #[test]
    fn oci_extra_roots_is_non_empty() {
        assert!(
            !oci_extra_roots().is_empty(),
            "embedded CA roots must be seeded, else the empty-store panic branch is reachable"
        );
    }

    /// Both seams draw from the same embedded set, so the direct `reqwest`
    /// clients and the `oci-client` registry client share identical trust
    /// anchors. Locks the DER→`reqwest::Certificate` projection against a
    /// silent parse-drop regression.
    #[test]
    fn reqwest_projection_covers_the_whole_embedded_set() {
        let embedded = webpki_root_certs::TLS_SERVER_ROOT_CERTS.len();
        assert_eq!(oci_extra_roots().len(), embedded);
        let reqwest_roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter(|der| reqwest::Certificate::from_der(der.as_ref()).is_ok())
            .count();
        assert_eq!(reqwest_roots, embedded);
    }
}
