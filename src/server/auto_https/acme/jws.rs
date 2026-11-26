use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openssl::bn::{BigNum, BigNumContext, BigNumRef};
use openssl::ec::{EcGroup, EcKey};
use openssl::ecdsa::EcdsaSig;
use openssl::hash::{MessageDigest, hash};
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::sign::Signer;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::Write;

pub(in crate::server) struct AccountKey {
    key: PKey<Private>,
    jwk: Value,
    thumbprint: String,
}

impl AccountKey {
    pub(in crate::server) fn load_or_create(cert_dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(cert_dir)?;
        let path = cert_dir.join("acme-account.key");
        let key = match std::fs::read(&path) {
            Ok(pem) => PKey::private_key_from_pem(&pem).map_err(openssl_error)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = generate_key()?;
                write_private_key(&path, &key)?;
                key
            }
            Err(error) => return Err(error),
        };
        Self::from_key(key)
    }

    fn from_key(key: PKey<Private>) -> io::Result<Self> {
        let ec = key.ec_key().map_err(openssl_error)?;
        let mut context = BigNumContext::new().map_err(openssl_error)?;
        let mut x = BigNum::new().map_err(openssl_error)?;
        let mut y = BigNum::new().map_err(openssl_error)?;
        ec.public_key()
            .affine_coordinates(ec.group(), &mut x, &mut y, &mut context)
            .map_err(openssl_error)?;
        let x = URL_SAFE_NO_PAD.encode(padded_coordinate(&x)?);
        let y = URL_SAFE_NO_PAD.encode(padded_coordinate(&y)?);
        let jwk = json!({"crv": "P-256", "kty": "EC", "x": x, "y": y});
        let canonical = serde_json::to_vec(&jwk)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let thumbprint = URL_SAFE_NO_PAD
            .encode(hash(MessageDigest::sha256(), &canonical).map_err(openssl_error)?);
        Ok(Self {
            key,
            jwk,
            thumbprint,
        })
    }

    pub(in crate::server) fn jwk(&self) -> &Value {
        &self.jwk
    }

    pub(in crate::server) fn thumbprint(&self) -> &str {
        &self.thumbprint
    }

    pub(in crate::server) fn sign_jws(
        &self,
        protected: Value,
        payload: &[u8],
    ) -> io::Result<Value> {
        let protected = serde_json::to_vec(&protected)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let protected = URL_SAFE_NO_PAD.encode(protected);
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{protected}.{payload}");
        let signature = sign_es256(&self.key, signing_input.as_bytes())?;
        Ok(json!({
            "protected": protected,
            "payload": payload,
            "signature": URL_SAFE_NO_PAD.encode(signature),
        }))
    }
}

fn generate_key() -> io::Result<PKey<Private>> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).map_err(openssl_error)?;
    let key = EcKey::generate(&group).map_err(openssl_error)?;
    PKey::from_ec_key(key).map_err(openssl_error)
}

fn padded_coordinate(value: &BigNumRef) -> io::Result<Vec<u8>> {
    let bytes = value.to_vec();
    if bytes.len() > 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "EC coordinate is too large",
        ));
    }
    let mut padded = vec![0_u8; 32 - bytes.len()];
    padded.extend_from_slice(&bytes);
    Ok(padded)
}

fn sign_es256(key: &PKey<Private>, message: &[u8]) -> io::Result<Vec<u8>> {
    let mut signer = Signer::new(MessageDigest::sha256(), key).map_err(openssl_error)?;
    signer.update(message).map_err(openssl_error)?;
    let der = signer.sign_to_vec().map_err(openssl_error)?;
    let signature = EcdsaSig::from_der(&der).map_err(openssl_error)?;
    let mut bytes = padded_coordinate(signature.r())?;
    bytes.extend_from_slice(&padded_coordinate(signature.s())?);
    Ok(bytes)
}

fn write_private_key(path: &Path, key: &PKey<Private>) -> io::Result<()> {
    let pem = key.private_key_to_pem_pkcs8().map_err(openssl_error)?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&pem)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn openssl_error(error: openssl::error::ErrorStack) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_key_has_stable_jwk_and_signature_shape() {
        let directory = std::env::temp_dir().join(format!(
            "starling-acme-key-{}-{}",
            std::process::id(),
            AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let first = AccountKey::load_or_create(&directory).unwrap();
        let second = AccountKey::load_or_create(&directory).unwrap();
        assert_eq!(first.jwk(), second.jwk());
        assert_eq!(first.thumbprint(), second.thumbprint());
        let jws = first
            .sign_jws(json!({"alg": "ES256", "nonce": "n", "url": "u"}), b"{}")
            .unwrap();
        let signature = jws["signature"].as_str().unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(signature).unwrap().len(), 64);
        let _ = std::fs::remove_dir_all(directory);
    }
}
