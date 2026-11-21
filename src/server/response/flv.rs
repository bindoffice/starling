use super::*;

const FLV_HEADER: &[u8; 13] = b"FLV\x01\x05\0\0\0\x09\0\0\0\0";

pub(in crate::server) async fn serve_flv(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    selected_location: Option<&LocationConfig>,
    variables: &Variables,
) -> Option<Response> {
    if !context.flv || env.uri.ends_with('/') {
        return None;
    }

    let path = map_request_path(context, selected_location, &env.uri);
    if let Err(status) = check_path_access(&path, context, variables).await {
        return Some(error_response(status));
    }
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return None,
        Err(_) => return Some(error_response(404)),
    };

    let size = metadata.len();
    let start = env
        .args
        .as_deref()
        .and_then(|args| query_argument(args, "start"))
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|start| *start < size)
        .unwrap_or(0);
    let mut response =
        match crate::http::file_response(path.clone(), content_type(&path, context), size) {
            Ok(response) => response,
            Err(_) => return Some(error_response(403)),
        };

    if start > 0 {
        let ResponseBody::File {
            file,
            offset: _,
            length: _,
        } = response.body
        else {
            return Some(error_response(500));
        };
        response.body = ResponseBody::FileWithPrefix {
            prefix: FLV_HEADER.to_vec(),
            file,
            offset: start,
            length: size - start,
        };
        response.set_content_length();
    }

    Some(
        finalize_static_response(request, response, &metadata, context)
            .unwrap_or_else(|| error_response(416)),
    )
}

fn query_argument<'a>(args: &'a str, name: &str) -> Option<&'a str> {
    args.split('&').find_map(|argument| {
        let (key, value) = argument.split_once('=').unwrap_or((argument, ""));
        (key == name).then_some(value)
    })
}
