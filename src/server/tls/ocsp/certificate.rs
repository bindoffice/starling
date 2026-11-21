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
        certificate_chain: &[CertificateDer<'static>],
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
        let mut chain = Stack::new().map_err(openssl_error)?;
        for certificate in certificate_chain.iter().skip(1) {
            chain
                .push(X509::from_der(certificate.as_ref()).map_err(openssl_error)?)
                .map_err(openssl_error)?;
        }
        let trusted = trusted_certificate
            .map(load_trusted_certificates)
            .transpose()?
            .unwrap_or_default();

        let mut issuer = None;
        for candidate in chain.iter() {
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

        let mut store = X509StoreBuilder::new().map_err(openssl_error)?;
        for certificate in trusted {
            store.add_cert(certificate).map_err(openssl_error)?;
        }
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
}

fn load_trusted_certificates(path: &Path) -> io::Result<Vec<X509>> {
    let certificates = load_certificates(path)?;
    certificates
        .into_iter()
        .map(|certificate| X509::from_der(certificate.as_ref()).map_err(openssl_error))
        .collect()
}

pub(super) fn openssl_error(error: openssl::error::ErrorStack) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
