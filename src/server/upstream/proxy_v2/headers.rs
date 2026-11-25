use super::*;

pub(super) struct H2ResponseHead {
    pub(super) parts: http::response::Parts,
    pub(super) body: h2::RecvStream,
    pub(super) early_hints: Vec<Vec<Header>>,
}

pub(super) async fn read_h2_response_head(
    mut response: h2::client::ResponseFuture,
) -> Result<H2ResponseHead, ProxyFailure> {
    let mut early_hints = Vec::new();
    loop {
        let informational =
            std::future::poll_fn(|context| response.poll_informational(context)).await;
        match informational {
            Some(Ok(response)) if response.status() == http::StatusCode::EARLY_HINTS => {
                early_hints.push(convert_headers(response.headers())?);
            }
            Some(Ok(_)) => {}
            Some(Err(_)) => return Err(ProxyFailure::InvalidHeader),
            None => break,
        }
    }
    let response = response.await.map_err(|_| ProxyFailure::InvalidHeader)?;
    let (parts, body) = response.into_parts();
    Ok(H2ResponseHead {
        parts,
        body,
        early_hints,
    })
}

pub(super) fn convert_headers(headers: &http::HeaderMap) -> Result<Vec<Header>, ProxyFailure> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = value.to_str().map_err(|_| ProxyFailure::InvalidHeader)?;
            Ok(Header {
                name: name.as_str().into(),
                value: value.to_string(),
            })
        })
        .collect()
}
