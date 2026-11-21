use super::*;

pub(in crate::server) fn stream_split_clients_value(
    split: &SplitClientsVariable,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
    captures: &StreamCaptures,
) -> String {
    if stack.iter().any(|name| name == &split.name) {
        return String::new();
    }
    stack.push(split.name.clone());

    let source = eval_stream_variables_with_stack(&split.source, log, stack, captures);
    let hash = nginx_murmur_hash2(source.as_bytes());
    let value = split
        .parts
        .iter()
        .find(|part| part.percent == 0 || hash < part.threshold)
        .map(|part| part.value.clone())
        .unwrap_or_default();
    let output = value;

    stack.pop();
    output
}
