use std::io;
use std::path::Path;

use openssl::hash::MessageDigest;
use openssl::ocsp::{OcspCertId, OcspRequest};
use openssl::stack::Stack;
use openssl::x509::X509;
use openssl::x509::store::{X509Store, X509StoreBuilder};

use super::*;

pub(super) struct CertificateIdentity {
    pub(super) leaf: X509,
    pub(super) issuer: X509,
    pub(super) chain: Stack<X509>,
    pub(super) store: X509Store,
    pub(super) verify: bool,
}

impl CertificateIdentity {
    pub(super) fn load(
        certificate_chain: &[CertificateDer<'_>],
        trusted_certificate: Option<&Path>,
        verify: bool,
    ) -> io::Result<Option<Self>> {
        let Some(leaf) = certificate_chain
            .first()
            .map(|certificate| X509::from_der(certificate.as_ref()))
            .transpose()
            .map_err(openssl_error)?
        else {
            return Ok(None);
        };
        let mut chain = Vec::new();
        for certificate in certificate_chain.iter().skip(1) {
            chain.push(X509::from_der(certificate.as_ref()).map_err(openssl_error)?);
        }
        let trusted = trusted_certificate
            .map(load_trusted_certificates)
            .transpose()?
            .unwrap_or_default();
        Self::from_parts(leaf, chain, trusted, verify)
    }

    pub(super) fn from_parts(
        leaf: X509,
        certificates: Vec<X509>,
        trusted: Vec<X509>,
        verify: bool,
    ) -> io::Result<Option<Self>> {
        let mut issuer = None;
        for candidate in &certificates {
            if candidate.issued(&leaf) == openssl::x509::X509VerifyResult::OK {
                issuer = Some(
                    X509::from_der(&candidate.to_der().map_err(openssl_error)?)
                        .map_err(openssl_error)?,
                );
                break;
            }
        }
        if issuer.is_none() {
            issuer = trusted
                .iter()
                .find(|candidate| candidate.issued(&leaf) == openssl::x509::X509VerifyResult::OK)
                .cloned();
        }
        let Some(issuer) = issuer else {
            return Ok(None);
        };

        let mut chain = Stack::new().map_err(openssl_error)?;
        for certificate in certificates {
            // A self-issued root is not an authorized OCSP response signer unless
            // it is also the issuer of the certificate being checked.
            if certificate.issued(&certificate) != openssl::x509::X509VerifyResult::OK {
                chain.push(certificate).map_err(openssl_error)?;
            }
        }
        // OCSP_TRUSTOTHER treats certificates in this stack as trusted. Include
        // the issuer, but not the rest of the CA bundle, so unrelated roots
        // cannot sign a response for this certificate.
        chain.push(issuer.clone()).map_err(openssl_error)?;
        let mut store = X509StoreBuilder::new().map_err(openssl_error)?;
        // OCSP response signers must be the certificate issuer or a responder
        // delegated by that issuer. Anchoring the store at the issuer prevents
        // an unrelated trusted root from signing a response for this leaf.
        store.add_cert(issuer.clone()).map_err(openssl_error)?;
        Ok(Some(Self {
            leaf,
            issuer,
            chain,
            store: store.build(),
            verify,
        }))
    }

    pub(super) fn responder(&self) -> io::Result<Option<String>> {
        let responders = self.leaf.ocsp_responders().map_err(openssl_error)?;
        Ok(responders.iter().next().map(ToString::to_string))
    }

    pub(super) fn request(&self) -> io::Result<Vec<u8>> {
        let mut request = OcspRequest::new().map_err(openssl_error)?;
        request
            .add_id(self.certificate_id()?)
            .map_err(openssl_error)?;
        request.to_der().map_err(openssl_error)
    }

    pub(super) fn certificate_id(&self) -> io::Result<OcspCertId> {
        OcspCertId::from_cert(MessageDigest::sha1(), &self.leaf, &self.issuer)
            .map_err(openssl_error)
    }

    pub(super) fn cache_key(&self, responder: &str) -> io::Result<Vec<u8>> {
        let mut key = self
            .leaf
            .digest(MessageDigest::sha256())
            .map_err(openssl_error)?
            .to_vec();
        key.extend_from_slice(responder.as_bytes());
        Ok(key)
    }
}

pub(super) fn load_trusted_certificates(path: &Path) -> io::Result<Vec<X509>> {
    let certificates = load_certificates(path)?;
    certificates
        .into_iter()
        .map(|certificate| X509::from_der(certificate.as_ref()).map_err(openssl_error))
        .collect()
}

pub(super) fn openssl_error(error: openssl::error::ErrorStack) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
