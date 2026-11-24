use super::*;

mod render;

#[derive(Debug)]
struct Entry {
    name: String,
    kind: EntryKind,
    modified: SystemTime,
    size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Other,
}

pub(in crate::server) async fn autoindex_response(
    request: &Request,
    path: &Path,
    uri: &str,
    context: &EffectiveConfig,
) -> Response {
    let callback = match context.autoindex_format {
        AutoindexFormat::Jsonp => match jsonp_callback(request) {
            Ok(callback) => callback,
            Err(()) => return error_response(400),
        },
        _ => None,
    };
    let format = if context.autoindex_format == AutoindexFormat::Jsonp && callback.is_none() {
        AutoindexFormat::Json
    } else {
        context.autoindex_format
    };
    let entries = match read_entries(path).await {
        Ok(entries) => entries,
        Err(error) => return autoindex_error(error),
    };
    render::render(
        format,
        uri,
        callback.as_deref(),
        &entries,
        context.autoindex_exact_size,
        context.autoindex_localtime,
    )
}

async fn read_entries(path: &Path) -> io::Result<Vec<Entry>> {
    let mut directory = tokio::fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = directory.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let metadata = match entry_metadata(&entry).await? {
            Some(metadata) => metadata,
            None => continue,
        };
        let (kind, modified, size) = (
            if metadata.is_dir() {
                EntryKind::Directory
            } else if metadata.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            },
            metadata.modified().unwrap_or(UNIX_EPOCH),
            metadata.len(),
        );
        entries.push(Entry {
            name,
            kind,
            modified,
            size,
        });
    }
    entries.sort_by(|left, right| {
        right
            .kind
            .eq(&EntryKind::Directory)
            .cmp(&left.kind.eq(&EntryKind::Directory))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}

async fn entry_metadata(entry: &tokio::fs::DirEntry) -> io::Result<Option<std::fs::Metadata>> {
    let path = entry.path();
    match tokio::fs::metadata(&path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ELOOP) =>
        {
            // Nginx follows links while building autoindex entries. Broken and
            // looping links fall back to link metadata and are shown as "other".
            tokio::fs::symlink_metadata(path).await.map(Some)
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(None),
        Err(error) => Err(error),
    }
}

fn jsonp_callback(request: &Request) -> Result<Option<String>, ()> {
    let Some(callback) = request
        .path_and_query()
        .1
        .and_then(|args| query_argument(args, "callback"))
    else {
        return Ok(None);
    };
    if callback.len() > 128 {
        return Err(());
    }
    if callback.is_empty() {
        return Ok(None);
    }
    if !callback
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
    {
        return Err(());
    }
    Ok(Some(callback.to_string()))
}

fn query_argument<'a>(args: &'a str, name: &str) -> Option<&'a str> {
    args.split('&').find_map(|argument| {
        let (key, value) = argument.split_once('=')?;
        key.eq_ignore_ascii_case(name).then_some(value)
    })
}

fn autoindex_error(error: io::Error) -> Response {
    let status = match error.kind() {
        io::ErrorKind::NotFound => 404,
        io::ErrorKind::PermissionDenied => 403,
        _ => 500,
    };
    error_response(status)
}
