use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openssl::hash::{MessageDigest, hash};
use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const DNS_HOOK_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_DNS_HOOK_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Dns01Challenge {
    pub(super) domain: String,
    pub(super) record_name: String,
    pub(super) record_value: String,
}

impl Dns01Challenge {
    pub(super) fn from_authorization(
        identifier: &str,
        wildcard: bool,
        key_authorization: &str,
    ) -> io::Result<Self> {
        let identifier = identifier.trim().trim_end_matches('.');
        let wildcard = wildcard || identifier.starts_with("*.");
        let base = identifier
            .strip_prefix("*.")
            .unwrap_or(identifier)
            .to_ascii_lowercase();
        if base.is_empty()
            || base
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace() || byte == b'/')
        {
            return Err(invalid_data(format!(
                "ACME DNS-01 authorization has an invalid identifier \"{identifier}\""
            )));
        }
        let digest =
            hash(MessageDigest::sha256(), key_authorization.as_bytes()).map_err(openssl_error)?;
        Ok(Self {
            domain: if wildcard {
                format!("*.{base}")
            } else {
                base.clone()
            },
            record_name: format!("_acme-challenge.{base}"),
            record_value: URL_SAFE_NO_PAD.encode(digest),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum DnsHookAction {
    Present,
    Cleanup,
}

impl DnsHookAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Cleanup => "cleanup",
        }
    }
}

pub(super) async fn run_dns_hook(
    hook: &Path,
    action: DnsHookAction,
    challenge: &Dns01Challenge,
) -> io::Result<()> {
    let action_name = action.as_str();
    let payload = serde_json::to_vec(&json!({
        "action": action_name,
        "name": challenge.record_name,
        "value": challenge.record_value,
        "domain": challenge.domain,
    }))
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut command = Command::new(hook);
    command
        .env("STARLING_ACME_DNS_ACTION", action_name)
        .env("STARLING_ACME_DNS_NAME", &challenge.record_name)
        .env("STARLING_ACME_DNS_VALUE", &challenge.record_value)
        .env("STARLING_ACME_DNS_DOMAIN", &challenge.domain)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to start ACME DNS-01 hook {} for {}: {error}",
                hook.display(),
                challenge.record_name
            ),
        )
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        // JSON on stdin is supplemental; hooks may use environment variables only.
        let _ = stdin.write_all(&payload).await;
        let _ = stdin.shutdown().await;
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let operation = async {
        let (status, stdout, stderr) = tokio::join!(
            child.wait(),
            read_dns_hook_output(stdout),
            read_dns_hook_output(stderr)
        );
        Ok::<_, io::Error>((status?, stdout?, stderr?))
    };
    let (status, stdout, stderr) = match tokio::time::timeout(DNS_HOOK_TIMEOUT, operation).await {
        Ok(result) => result?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "ACME DNS-01 hook {action_name} timed out after {} seconds for {}",
                    DNS_HOOK_TIMEOUT.as_secs(),
                    challenge.record_name
                ),
            ));
        }
    };
    if status.success() {
        return Ok(());
    }
    let detail = if stderr.is_empty() { stdout } else { stderr };
    let detail = String::from_utf8_lossy(&detail);
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    Err(io::Error::other(format!(
        "ACME DNS-01 hook {action_name} failed for {} with {status}{detail}",
        challenge.record_name
    )))
}

async fn read_dns_hook_output(reader: Option<impl AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    reader
        .take((MAX_DNS_HOOK_OUTPUT + 1) as u64)
        .read_to_end(&mut output)
        .await?;
    if output.len() > MAX_DNS_HOOK_OUTPUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ACME DNS-01 hook output is too large",
        ));
    }
    Ok(output)
}
