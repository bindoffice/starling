use super::*;

pub(in crate::server) fn append_cgi_request_headers(
    params: &mut Vec<(String, String)>,
    headers: &[Header],
) {
    let configured: HashSet<String> = params
        .iter()
        .map(|(name, _)| name.to_ascii_uppercase())
        .collect();
    let mut added: HashMap<String, usize> = HashMap::new();
    for header in headers {
        let name = cgi_request_header_name(&header.name);
        if configured.contains(&name) {
            continue;
        }
        if let Some(index) = added.get(&name) {
            let separator = if header.name.eq_ignore_ascii_case("cookie") {
                "; "
            } else {
                ", "
            };
            params[*index].1.push_str(separator);
            params[*index].1.push_str(&header.value);
        } else {
            added.insert(name.clone(), params.len());
            params.push((name, header.value.clone()));
        }
    }
}

pub(in crate::server) fn cgi_request_header_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 5);
    result.push_str("HTTP_");
    for byte in name.bytes() {
        result.push(if byte == b'-' {
            '_'
        } else {
            byte.to_ascii_uppercase() as char
        });
    }
    result
}
