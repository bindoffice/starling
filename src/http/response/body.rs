use super::*;

#[derive(Debug)]
pub enum ResponseBody {
    Empty,
    Bytes(Vec<u8>),
    File {
        file: File,
        offset: u64,
        length: u64,
    },
    FileWithPrefix {
        prefix: Vec<u8>,
        file: File,
        offset: u64,
        length: u64,
    },
    Segments {
        source: Box<ResponseBody>,
        segments: Vec<BodySegment>,
    },
    Stream(ResponseStream),
}

#[derive(Debug)]
pub struct BodySegment {
    pub prefix: Vec<u8>,
    pub start: u64,
    pub end: u64,
}

impl BodySegment {
    pub(super) fn len(&self) -> u64 {
        self.prefix
            .len()
            .try_into()
            .unwrap_or(u64::MAX)
            .saturating_add(self.end.saturating_sub(self.start))
    }
}

impl ResponseBody {
    pub fn len(&self) -> u64 {
        self.content_length().unwrap_or(0)
    }

    pub(crate) fn content_length(&self) -> Option<u64> {
        match self {
            Self::Empty => Some(0),
            Self::Bytes(body) => Some(body.len() as u64),
            Self::File { length, .. } => Some(*length),
            Self::FileWithPrefix { prefix, length, .. } => {
                Some((prefix.len() as u64).saturating_add(*length))
            }
            Self::Segments { segments, .. } => segments
                .iter()
                .try_fold(0_u64, |total, segment| total.checked_add(segment.len())),
            Self::Stream(stream) => stream.length(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !matches!(self, Self::Stream(_)) && self.content_length() == Some(0)
    }

    pub(crate) fn is_stream(&self) -> bool {
        matches!(self, Self::Stream(_))
    }

    pub(crate) fn stream_may_have_trailers(&self) -> bool {
        matches!(self, Self::Stream(stream) if stream.may_have_trailers())
    }

    pub(crate) fn preserve_on_client_abort(&self) -> bool {
        match self {
            Self::Stream(stream) => stream.preserve_on_client_abort(),
            Self::Segments { source, .. } => source.preserve_on_client_abort(),
            _ => false,
        }
    }

    pub(crate) fn mark_preserve_on_client_abort(&mut self) {
        match self {
            Self::Stream(stream) => stream.set_preserve_on_client_abort(),
            Self::Segments { source, .. } => source.mark_preserve_on_client_abort(),
            _ => {}
        }
    }

    pub(crate) async fn drain_stream(self) {
        match self {
            Self::Stream(stream) => stream.drain().await,
            Self::Segments { source, .. } => Box::pin(source.drain_stream()).await,
            _ => {}
        }
    }

    pub async fn into_memory(self, limit: u64) -> Result<Vec<u8>, Box<Self>> {
        match self {
            Self::Empty => Ok(Vec::new()),
            Self::Bytes(body) => {
                if body.len() as u64 <= limit {
                    Ok(body)
                } else {
                    Err(Box::new(Self::Bytes(body)))
                }
            }
            Self::File {
                mut file,
                offset,
                length,
            } => {
                if length > limit {
                    return Err(Box::new(Self::File {
                        file,
                        offset,
                        length,
                    }));
                }
                if file.seek(SeekFrom::Start(offset)).await.is_err() {
                    return Err(Box::new(Self::File {
                        file,
                        offset,
                        length,
                    }));
                }
                let mut body = vec![0; length as usize];
                if file.read_exact(&mut body).await.is_err() {
                    return Err(Box::new(Self::File {
                        file,
                        offset,
                        length,
                    }));
                }
                Ok(body)
            }
            Self::FileWithPrefix {
                prefix,
                mut file,
                offset,
                length,
            } => {
                let total = (prefix.len() as u64).saturating_add(length);
                if total > limit {
                    return Err(Box::new(Self::FileWithPrefix {
                        prefix,
                        file,
                        offset,
                        length,
                    }));
                }
                if file.seek(SeekFrom::Start(offset)).await.is_err() {
                    return Err(Box::new(Self::FileWithPrefix {
                        prefix,
                        file,
                        offset,
                        length,
                    }));
                }
                let mut body = Vec::with_capacity(total as usize);
                body.extend_from_slice(&prefix);
                let start = body.len();
                body.resize(start + length as usize, 0);
                if file.read_exact(&mut body[start..]).await.is_err() {
                    return Err(Box::new(Self::FileWithPrefix {
                        prefix,
                        file,
                        offset,
                        length,
                    }));
                }
                Ok(body)
            }
            Self::Segments { source, segments } => {
                let total = segments
                    .iter()
                    .fold(0_u64, |total, segment| total.saturating_add(segment.len()));
                if total > limit {
                    return Err(Box::new(Self::Segments { source, segments }));
                }
                let source = match Box::pin(source.into_memory(limit)).await {
                    Ok(source) => source,
                    Err(source) => {
                        return Err(Box::new(Self::Segments { source, segments }));
                    }
                };
                let Ok(capacity) = usize::try_from(total) else {
                    return Err(Box::new(Self::Bytes(source)));
                };
                let mut body = Vec::with_capacity(capacity);
                for segment in segments {
                    body.extend_from_slice(&segment.prefix);
                    body.extend_from_slice(&source[segment.start as usize..segment.end as usize]);
                }
                Ok(body)
            }
            Self::Stream(mut stream) => {
                let mut body = Vec::new();
                loop {
                    let item = match stream.next().await {
                        Ok(item) => item,
                        Err(_) => return Err(Box::new(Self::Bytes(body))),
                    };
                    let Some(item) = item else {
                        break;
                    };
                    let ResponseStreamItem::Data(data) = item else {
                        continue;
                    };
                    if body.len().saturating_add(data.len()) > limit as usize {
                        return Err(Box::new(Self::Bytes(body)));
                    }
                    body.extend_from_slice(&data);
                }
                Ok(body)
            }
        }
    }
}
