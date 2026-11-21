use super::*;

pub(in crate::server) fn response_header_value(headers: &[Header], name: &str) -> String {
    let header_name = name.replace('_', "-");
    let underscored_name = header_name.replace('-', "_");
    let mut values = headers.iter().filter_map(|header| {
        (header.name.eq_ignore_ascii_case(&header_name)
            || header.name.eq_ignore_ascii_case(&underscored_name))
        .then_some(header.value.as_str())
    });
    let Some(first) = values.next() else {
        return String::new();
    };
    let mut value = first.to_string();
    for next in values {
        value.push_str(", ");
        value.push_str(next);
    }
    value
}
