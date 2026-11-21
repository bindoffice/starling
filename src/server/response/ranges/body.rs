use super::super::*;

pub(super) fn slice(body: &mut ResponseBody, offset: u64, length: u64) {
    let current = std::mem::replace(body, ResponseBody::Empty);
    *body = match current {
        ResponseBody::Empty => ResponseBody::Empty,
        ResponseBody::Bytes(data) => {
            let start = offset as usize;
            let end = start + length as usize;
            ResponseBody::Bytes(data[start..end].to_vec())
        }
        ResponseBody::File {
            file,
            offset: file_offset,
            ..
        } => ResponseBody::File {
            file,
            offset: file_offset.saturating_add(offset),
            length,
        },
        ResponseBody::FileWithPrefix {
            prefix,
            file,
            offset: file_offset,
            ..
        } => {
            let end = offset.saturating_add(length);
            let prefix_len = prefix.len() as u64;
            if end <= prefix_len {
                ResponseBody::Bytes(prefix[offset as usize..end as usize].to_vec())
            } else if offset >= prefix_len {
                ResponseBody::File {
                    file,
                    offset: file_offset.saturating_add(offset - prefix_len),
                    length,
                }
            } else {
                ResponseBody::FileWithPrefix {
                    prefix: prefix[offset as usize..].to_vec(),
                    file,
                    offset: file_offset,
                    length: end - prefix_len,
                }
            }
        }
        segments @ ResponseBody::Segments { .. } => segments,
        stream @ ResponseBody::Stream(_) => stream,
    };
}
