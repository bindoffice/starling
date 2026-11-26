use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::x509::extension::SubjectAlternativeName;
use openssl::x509::{X509NameBuilder, X509ReqBuilder};
use serde_json::{Value, json};

mod dns;
mod http;
mod jws;
mod manager;

use dns::{Dns01Challenge, DnsHookAction, run_dns_hook};
use http::{AcmeHttpClient, AcmeHttpResponse};
use jws::AccountKey;
pub(in crate::server) use manager::auto_https_manager_loop;

const MAX_POLL_TIME: Duration = Duration::from_secs(120);

pub(in crate::server) async fn obtain_certificate(
    order: &AcmeOrder,
    state: &AutoHttpsState,
    logger: Option<&Logger>,
) -> io::Result<()> {
    let account_key = AccountKey::load_or_create(&certificate_dir(&order.paths))?;
    let http = AcmeHttpClient::new(order.ca_file.as_deref())?;
    let directory = fetch_directory(&http, &order.directory_url).await?;
    let mut session = AcmeSession {
        http,
        directory,
        account_key,
        kid: None,
        nonce: None,
    };
    session.ensure_account(order.email.as_deref()).await?;
    let (order_url, order_body) = session.new_order(&order.domains).await?;
    let mut order_status = order_body["status"]
        .as_str()
        .unwrap_or("pending")
        .to_string();
    let authorizations = order_body["authorizations"]
        .as_array()
        .ok_or_else(|| invalid_data("ACME order has no authorizations"))?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    match order.challenge {
        AutoHttpsChallenge::Http01 => {
            for authorization in authorizations {
                session
                    .complete_http01_authorization(&authorization, state)
                    .await?;
            }
        }
        AutoHttpsChallenge::Dns01 => {
            session
                .complete_dns01_authorizations(&authorizations, order, logger)
                .await?;
        }
    }

    if order_status == "pending" {
        order_status = session
            .poll_order(&order_url, &["ready", "valid"], MAX_POLL_TIME)
            .await?;
    }
    if order_status != "valid" {
        let leaf_key = generate_leaf_key()?;
        let csr = generate_csr(&leaf_key, &order.domains)?;
        let body = session
            .post_signed(
                order_body["finalize"]
                    .as_str()
                    .ok_or_else(|| invalid_data("ACME order has no finalize URL"))?,
                Some(&json!({"csr": URL_SAFE_NO_PAD.encode(csr)})),
            )
            .await?;
        if body.status != 200 && body.status != 201 {
            return Err(response_error("ACME finalize", &body));
        }
        session
            .poll_order(&order_url, &["valid"], MAX_POLL_TIME)
            .await?;
        let order_response = session.post_as_get(&order_url).await?;
        let order_json = parse_json(&order_response)?;
        let certificate_url = order_json["certificate"]
            .as_str()
            .ok_or_else(|| invalid_data("ACME order has no certificate URL"))?;
        let certificate = session.post_as_get(certificate_url).await?;
        if certificate.status != 200 {
            return Err(response_error("ACME certificate download", &certificate));
        }
        write_acme_certificate(
            &order.paths,
            &certificate.body,
            &leaf_key.private_key_to_pem_pkcs8().map_err(openssl_error)?,
        )?;
    } else {
        return Err(invalid_data(
            "ACME returned a valid order without a downloadable certificate",
        ));
    }
    Ok(())
}

struct PendingChallenge {
    authorization_url: String,
    challenge_url: String,
    token: String,
    key_authorization: String,
    identifier: String,
    wildcard: bool,
}

struct AcmeDirectory {
    new_nonce: String,
    new_account: String,
    new_order: String,
}

async fn fetch_directory(http: &AcmeHttpClient, url: &str) -> io::Result<AcmeDirectory> {
    let response = http.request("GET", url, None, &[]).await?;
    if response.status != 200 {
        return Err(response_error("ACME directory", &response));
    }
    let value = parse_json(&response)?;
    Ok(AcmeDirectory {
        new_nonce: required_url(&value, "newNonce")?,
        new_account: required_url(&value, "newAccount")?,
        new_order: required_url(&value, "newOrder")?,
    })
}

struct AcmeSession {
    http: AcmeHttpClient,
    directory: AcmeDirectory,
    account_key: AccountKey,
    kid: Option<String>,
    nonce: Option<String>,
}

impl AcmeSession {
    async fn ensure_account(&mut self, email: Option<&str>) -> io::Result<()> {
        let mut payload = json!({"termsOfServiceAgreed": true});
        if let Some(email) = email.filter(|email| !email.is_empty()) {
            payload["contact"] = json!([format!("mailto:{email}")]);
        }
        let response = self
            .post_jwk(&self.directory.new_account.clone(), Some(&payload))
            .await?;
        if response.status != 200 && response.status != 201 {
            return Err(response_error("ACME account", &response));
        }
        let kid = response
            .header("location")
            .ok_or_else(|| invalid_data("ACME account response has no Location header"))?;
        self.kid = Some(kid.to_string());
        Ok(())
    }

    async fn new_order(&mut self, domains: &[String]) -> io::Result<(String, Value)> {
        let identifiers = domains
            .iter()
            .map(|domain| json!({"type": "dns", "value": domain}))
            .collect::<Vec<_>>();
        let response = self
            .post_signed(
                &self.directory.new_order.clone(),
                Some(&json!({"identifiers": identifiers})),
            )
            .await?;
        if response.status != 201 {
            return Err(response_error("ACME order", &response));
        }
        let url = response
            .header("location")
            .ok_or_else(|| invalid_data("ACME order response has no Location header"))?;
        Ok((url.to_string(), parse_json(&response)?))
    }

    async fn complete_http01_authorization(
        &mut self,
        authorization_url: &str,
        state: &AutoHttpsState,
    ) -> io::Result<()> {
        let Some(challenge) = self.pending_challenge(authorization_url, "http-01").await? else {
            return Ok(());
        };
        state.set_challenge(&challenge.token, challenge.key_authorization.clone());
        let result = async {
            self.trigger_challenge(&challenge, "HTTP-01").await?;
            self.wait_for_authorization(authorization_url, "HTTP-01")
                .await
        }
        .await;
        state.remove_challenge(&challenge.token);
        result
    }

    async fn complete_dns01_authorizations(
        &mut self,
        authorization_urls: &[String],
        order: &AcmeOrder,
        logger: Option<&Logger>,
    ) -> io::Result<()> {
        let hook = order.dns_hook.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "ACME DNS-01 challenge requires a dns_hook",
            )
        })?;
        let mut pending = Vec::new();
        for authorization_url in authorization_urls {
            let Some(challenge) = self.pending_challenge(authorization_url, "dns-01").await? else {
                continue;
            };
            let dns = Dns01Challenge::from_authorization(
                &challenge.identifier,
                challenge.wildcard,
                &challenge.key_authorization,
            )?;
            pending.push((challenge, dns));
        }
        if pending.is_empty() {
            return Ok(());
        }

        let mut attempted = Vec::with_capacity(pending.len());
        for (_, dns) in &pending {
            attempted.push(dns.clone());
            if let Err(error) = run_dns_hook(hook, DnsHookAction::Present, dns).await {
                cleanup_dns_records(hook, &attempted, logger).await;
                return Err(error);
            }
        }
        if !order.dns_propagation_delay.is_zero() {
            tokio::time::sleep(order.dns_propagation_delay).await;
        }

        let result = async {
            for (challenge, _) in &pending {
                self.trigger_challenge(challenge, "DNS-01").await?;
            }
            for (challenge, _) in &pending {
                self.wait_for_authorization(&challenge.authorization_url, "DNS-01")
                    .await?;
            }
            Ok(())
        }
        .await;
        cleanup_dns_records(hook, &attempted, logger).await;
        result
    }

    async fn pending_challenge(
        &mut self,
        authorization_url: &str,
        challenge_type: &str,
    ) -> io::Result<Option<PendingChallenge>> {
        let response = self.post_as_get(authorization_url).await?;
        if response.status != 200 {
            return Err(response_error("ACME authorization", &response));
        }
        let authorization = parse_json(&response)?;
        match authorization["status"].as_str() {
            Some("valid") => return Ok(None),
            Some("pending") | None => {}
            Some(status) => {
                return Err(invalid_data(format!(
                    "ACME authorization is {status}: {}",
                    response.body_text()
                )));
            }
        }
        let challenge = authorization["challenges"]
            .as_array()
            .and_then(|challenges| {
                challenges
                    .iter()
                    .find(|challenge| challenge["type"].as_str() == Some(challenge_type))
            })
            .ok_or_else(|| {
                invalid_data(format!(
                    "ACME authorization has no {} challenge",
                    challenge_type.to_ascii_uppercase()
                ))
            })?;
        let challenge_url = required_url(challenge, "url")?;
        let token = challenge["token"].as_str().ok_or_else(|| {
            invalid_data(format!(
                "ACME {} challenge has no token",
                challenge_type.to_ascii_uppercase()
            ))
        })?;
        let identifier = authorization["identifier"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let wildcard = authorization["wildcard"].as_bool().unwrap_or(false);
        Ok(Some(PendingChallenge {
            authorization_url: authorization_url.to_string(),
            challenge_url,
            token: token.to_string(),
            key_authorization: format!("{token}.{}", self.account_key.thumbprint()),
            identifier,
            wildcard,
        }))
    }

    async fn trigger_challenge(
        &mut self,
        challenge: &PendingChallenge,
        challenge_name: &str,
    ) -> io::Result<()> {
        let response = self
            .post_signed(&challenge.challenge_url, Some(&json!({})))
            .await?;
        if response.status != 200 {
            return Err(response_error(
                &format!("ACME {challenge_name} challenge"),
                &response,
            ));
        }
        Ok(())
    }

    async fn wait_for_authorization(
        &mut self,
        authorization_url: &str,
        challenge_name: &str,
    ) -> io::Result<()> {
        let deadline = Instant::now() + MAX_POLL_TIME;
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for ACME {challenge_name} authorization"),
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            let response = self.post_as_get(authorization_url).await?;
            if response.status != 200 {
                return Err(response_error("ACME authorization poll", &response));
            }
            let value = parse_json(&response)?;
            match value["status"].as_str() {
                Some("valid") => return Ok(()),
                Some("pending") | Some("processing") | None => {}
                Some(status) => {
                    return Err(invalid_data(format!(
                        "ACME {challenge_name} authorization failed with status {status}: {}",
                        response.body_text()
                    )));
                }
            }
        }
    }

    async fn poll_order(
        &mut self,
        order_url: &str,
        accepted: &[&str],
        timeout: Duration,
    ) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let response = self.post_as_get(order_url).await?;
            if response.status != 200 {
                return Err(response_error("ACME order poll", &response));
            }
            let value = parse_json(&response)?;
            let status = value["status"].as_str().unwrap_or("pending");
            if accepted.contains(&status) {
                return Ok(status.to_string());
            }
            if matches!(status, "invalid" | "revoked") {
                return Err(invalid_data(format!(
                    "ACME order failed with status {status}: {}",
                    response.body_text()
                )));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for ACME order status {accepted:?}"),
                ));
            }
            tokio::time::sleep(retry_delay(&response)).await;
        }
    }

    async fn post_as_get(&mut self, url: &str) -> io::Result<AcmeHttpResponse> {
        self.post_signed(url, None).await
    }

    async fn post_signed(
        &mut self,
        url: &str,
        payload: Option<&Value>,
    ) -> io::Result<AcmeHttpResponse> {
        let payload = payload
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .unwrap_or_default();
        for attempt in 0..2 {
            let nonce = self.take_nonce().await?;
            let mut protected = json!({
                "alg": "ES256",
                "nonce": nonce,
                "url": url,
            });
            if let Some(kid) = &self.kid {
                protected["kid"] = Value::String(kid.clone());
            } else {
                protected["jwk"] = self.account_key.jwk().clone();
            }
            let jws = self.account_key.sign_jws(protected, &payload)?;
            let body = serde_json::to_vec(&jws)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let response = self
                .http
                .request("POST", url, Some("application/jose+json"), &body)
                .await?;
            self.remember_nonce(&response);
            if attempt == 0 && is_bad_nonce(&response) {
                continue;
            }
            return Ok(response);
        }
        unreachable!("nonce retry is bounded")
    }

    async fn post_jwk(
        &mut self,
        url: &str,
        payload: Option<&Value>,
    ) -> io::Result<AcmeHttpResponse> {
        self.post_signed(url, payload).await
    }

    async fn take_nonce(&mut self) -> io::Result<String> {
        if let Some(nonce) = self.nonce.take() {
            return Ok(nonce);
        }
        let response = self
            .http
            .request("HEAD", &self.directory.new_nonce, None, &[])
            .await?;
        if response.status != 200 && response.status != 204 {
            return Err(response_error("ACME nonce", &response));
        }
        response
            .header("replay-nonce")
            .map(str::to_string)
            .ok_or_else(|| invalid_data("ACME nonce response has no Replay-Nonce header"))
    }

    fn remember_nonce(&mut self, response: &AcmeHttpResponse) {
        if let Some(nonce) = response.header("replay-nonce") {
            self.nonce = Some(nonce.to_string());
        }
    }
}

async fn cleanup_dns_records(hook: &Path, records: &[Dns01Challenge], logger: Option<&Logger>) {
    for challenge in records.iter().rev() {
        if let Err(error) = run_dns_hook(hook, DnsHookAction::Cleanup, challenge).await
            && let Some(logger) = logger
        {
            logger.log(
                LogLevel::Warn,
                format!(
                    "failed to clean up ACME DNS-01 record {}: {error}",
                    challenge.record_name
                ),
            );
        }
    }
}

fn generate_leaf_key() -> io::Result<PKey<Private>> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).map_err(openssl_error)?;
    let key = EcKey::generate(&group).map_err(openssl_error)?;
    PKey::from_ec_key(key).map_err(openssl_error)
}

fn generate_csr(key: &PKey<Private>, domains: &[String]) -> io::Result<Vec<u8>> {
    let mut builder = X509ReqBuilder::new().map_err(openssl_error)?;
    builder.set_pubkey(key).map_err(openssl_error)?;
    let mut subject = X509NameBuilder::new().map_err(openssl_error)?;
    let common_name = domains
        .first()
        .map(String::as_str)
        .unwrap_or("")
        .chars()
        .take(64)
        .collect::<String>();
    subject
        .append_entry_by_nid(Nid::COMMONNAME, &common_name)
        .map_err(openssl_error)?;
    let subject = subject.build();
    builder.set_subject_name(&subject).map_err(openssl_error)?;
    let mut san = SubjectAlternativeName::new();
    for domain in domains {
        san.dns(domain);
    }
    let extension = {
        let context = builder.x509v3_context(None);
        san.build(&context).map_err(openssl_error)?
    };
    let mut extensions = openssl::stack::Stack::new().map_err(openssl_error)?;
    extensions.push(extension).map_err(openssl_error)?;
    builder.add_extensions(&extensions).map_err(openssl_error)?;
    builder
        .sign(key, MessageDigest::sha256())
        .map_err(openssl_error)?;
    builder.build().to_der().map_err(openssl_error)
}

fn certificate_dir(paths: &CertificatePaths) -> PathBuf {
    paths
        .cert
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn parse_json(response: &AcmeHttpResponse) -> io::Result<Value> {
    serde_json::from_slice(&response.body).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid ACME JSON response: {error}"),
        )
    })
}

fn required_url(value: &Value, name: &str) -> io::Result<String> {
    value[name]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| invalid_data(format!("ACME response has no {name} URL")))
}

fn is_bad_nonce(response: &AcmeHttpResponse) -> bool {
    response.status == 400
        && std::str::from_utf8(&response.body).is_ok_and(|body| body.contains("badNonce"))
}

fn response_error(context: &str, response: &AcmeHttpResponse) -> io::Error {
    io::Error::other(format!(
        "{context} failed with HTTP {}: {}",
        response.status,
        response.body_text()
    ))
}

fn retry_delay(response: &AcmeHttpResponse) -> Duration {
    response
        .header("retry-after")
        .and_then(parse_retry_after)
        .map(|seconds| Duration::from_secs(seconds.clamp(1, 10)))
        .unwrap_or(Duration::from_secs(2))
}

fn parse_retry_after(value: &str) -> Option<u64> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    retry_at
        .duration_since(SystemTime::now())
        .ok()
        .map(|delay| delay.as_secs())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

trait ResponseBodyText {
    fn body_text(&self) -> String;
}

impl ResponseBodyText for AcmeHttpResponse {
    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn openssl_error(error: openssl::error::ErrorStack) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests;
