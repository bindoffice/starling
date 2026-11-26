use super::*;

const NAME_LENGTH: usize = 50;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(super) fn render(
    format: AutoindexFormat,
    uri: &str,
    callback: Option<&str>,
    entries: &[Entry],
    exact_size: bool,
    localtime: bool,
) -> Response {
    match format {
        AutoindexFormat::Html => {
            Response::html(200, render_html(uri, entries, exact_size, localtime))
        }
        AutoindexFormat::Json => {
            Response::bytes(200, "application/json", render_json(entries, callback))
        }
        AutoindexFormat::Jsonp => Response::bytes(
            200,
            "application/javascript",
            render_json(entries, callback),
        ),
        AutoindexFormat::Xml => {
            Response::bytes(200, "text/xml; charset=utf-8", render_xml(entries))
        }
    }
}

fn render_html(uri: &str, entries: &[Entry], exact_size: bool, localtime: bool) -> String {
    let escaped_uri = html_escape(uri);
    let mut body = format!(
        "<html>\r\n<head><title>Index of {escaped_uri}</title></head>\r\n\
         <body>\r\n<h1>Index of {escaped_uri}</h1><hr><pre>\
         <a href=\"../\">../</a>\r\n"
    );
    for entry in entries {
        let display = display_name(entry);
        let href = percent_encode_path_segment(&display);
        let character_len = display.chars().count();
        body.push_str("<a href=\"");
        body.push_str(&href);
        body.push_str("\">");
        if character_len > NAME_LENGTH {
            let visible: String = display.chars().take(NAME_LENGTH - 3).collect();
            body.push_str(&html_escape(&visible));
            body.push_str("..&gt;</a>");
        } else {
            body.push_str(&html_escape(&display));
            body.push_str("</a>");
            body.push_str(&" ".repeat(NAME_LENGTH - character_len));
        }
        body.push(' ');
        body.push_str(&autoindex_time(entry.modified, localtime));
        body.push(' ');
        body.push_str(&html_size(entry, exact_size));
        body.push_str("\r\n");
    }
    body.push_str("</pre><hr></body>\r\n</html>\r\n");
    body
}

fn render_json(entries: &[Entry], callback: Option<&str>) -> Vec<u8> {
    let mut body = String::new();
    if let Some(callback) = callback {
        body.push_str("/* callback */\r\n");
        body.push_str(callback);
        body.push('(');
    }
    body.push('[');
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            body.push(',');
        }
        body.push_str("\r\n{ \"name\":\"");
        body.push_str(&json_escape(&entry.name));
        body.push_str("\", \"type\":\"");
        body.push_str(entry_kind(entry));
        body.push_str("\", \"mtime\":\"");
        body.push_str(&httpdate::fmt_http_date(entry.modified));
        body.push('"');
        if entry.kind == EntryKind::File {
            body.push_str(", \"size\":");
            body.push_str(&entry.size.to_string());
        }
        body.push_str(" }");
    }
    body.push_str("\r\n]");
    if callback.is_some() {
        body.push_str(");");
    }
    body.into_bytes()
}

fn render_xml(entries: &[Entry]) -> Vec<u8> {
    let mut body = String::from("<?xml version=\"1.0\"?>\r\n<list>\r\n");
    for entry in entries {
        let kind = entry_kind(entry);
        body.push('<');
        body.push_str(kind);
        body.push_str(" mtime=\"");
        body.push_str(&xml_time(entry.modified));
        body.push('"');
        if entry.kind == EntryKind::File {
            body.push_str(" size=\"");
            body.push_str(&entry.size.to_string());
            body.push('"');
        }
        body.push('>');
        body.push_str(&html_escape(&entry.name));
        body.push_str("</");
        body.push_str(kind);
        body.push_str(">\r\n");
    }
    body.push_str("</list>\r\n");
    body.into_bytes()
}

fn display_name(entry: &Entry) -> String {
    if entry.kind == EntryKind::Directory {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    }
}

fn entry_kind(entry: &Entry) -> &'static str {
    match entry.kind {
        EntryKind::Directory => "directory",
        EntryKind::File => "file",
        EntryKind::Other => "other",
    }
}

fn html_size(entry: &Entry, exact_size: bool) -> String {
    if entry.kind != EntryKind::File {
        return if exact_size {
            format!("{:>19}", "-")
        } else {
            format!("{:>7}", "-")
        };
    }
    if exact_size {
        format!("{:>19}", entry.size)
    } else {
        human_size(entry.size)
    }
}

fn human_size(length: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    let (value, scale) = if length > GIB - 1 {
        (rounded_size(length, GIB), Some('G'))
    } else if length > MIB - 1 {
        (rounded_size(length, MIB), Some('M'))
    } else if length > 9999 {
        (rounded_size(length, KIB), Some('K'))
    } else {
        (length, None)
    };
    match scale {
        Some(scale) => format!("{value:>6}{scale}"),
        None => format!(" {value:>6}"),
    }
}

fn rounded_size(length: u64, unit: u64) -> u64 {
    let whole = length / unit;
    if length % unit > unit / 2 - 1 {
        whole + 1
    } else {
        whole
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\x08' => escaped.push_str("\\b"),
            '\x0c' => escaped.push_str("\\f"),
            '\0'..='\x1f' => {
                escaped.push_str(&format!("\\u00{:02X}", character as u32));
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn xml_time(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    // SAFETY: libc::tm consists of integer and pointer fields, so its all-zero
    // representation is valid.
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers reference live, correctly initialized storage for
    // the duration of the call.
    unsafe {
        libc::gmtime_r(&seconds, &mut value);
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.tm_year + 1900,
        value.tm_mon + 1,
        value.tm_mday,
        value.tm_hour,
        value.tm_min,
        value.tm_sec
    )
}

fn autoindex_time(time: SystemTime, localtime: bool) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    // SAFETY: libc::tm consists of integer and pointer fields, so its all-zero
    // representation is valid.
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    if localtime {
        // SAFETY: both pointers reference live, correctly initialized storage
        // for the duration of the call.
        unsafe {
            libc::localtime_r(&seconds, &mut value);
        }
    } else {
        // SAFETY: both pointers reference live, correctly initialized storage
        // for the duration of the call.
        unsafe {
            libc::gmtime_r(&seconds, &mut value);
        }
    }
    let month = MONTHS
        .get(value.tm_mon.clamp(0, 11) as usize)
        .copied()
        .unwrap_or("Jan");
    format!(
        "{:02}-{}-{:04} {:02}:{:02}",
        value.tm_mday,
        month,
        value.tm_year + 1900,
        value.tm_hour,
        value.tm_min
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry {
            name: name.into(),
            kind,
            modified: UNIX_EPOCH,
            size,
        }
    }

    #[test]
    fn json_autoindex_escapes_names_and_preserves_utf8() {
        let entries = [
            entry("quote\"slash\\.txt", EntryKind::File, 42),
            entry("目录", EntryKind::Directory, 0),
        ];
        let body = String::from_utf8(render_json(&entries, None)).unwrap();
        assert!(body.contains("\"name\":\"quote\\\"slash\\\\.txt\""));
        assert!(body.contains("\"name\":\"目录\""));
        assert!(body.contains("\"type\":\"directory\""));
        assert!(body.contains("\"size\":42"));
    }

    #[test]
    fn xml_autoindex_formats_epoch_and_file_attributes() {
        let entries = [entry("a.txt", EntryKind::File, 7)];
        let body = String::from_utf8(render_xml(&entries)).unwrap();
        assert!(body.contains("<file mtime=\"1970-01-01T00:00:00Z\" size=\"7\">"));
        assert!(body.ends_with("</list>\r\n"));
    }

    #[test]
    fn jsonp_wraps_json_with_the_comment_and_callback() {
        let body = String::from_utf8(render_json(&[], Some("callback_1"))).unwrap();
        assert_eq!(body, "/* callback */\r\ncallback_1([\r\n]);");
    }
}
