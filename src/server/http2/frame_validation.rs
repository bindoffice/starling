use super::hpack_validation::HeaderBlockValidator;
use super::*;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(test)]
mod tests;

const HTTP2_PREFACE_LEN: usize = 24;
const FRAME_HEADER_LEN: usize = 9;
const DEFAULT_MAX_FRAME_SIZE: usize = 16 * 1024;

const FRAME_PRIORITY: u8 = 0x2;
const FRAME_RST_STREAM: u8 = 0x3;
const FRAME_SETTINGS: u8 = 0x4;
const FRAME_PING: u8 = 0x6;
const FRAME_GOAWAY: u8 = 0x7;
const FRAME_WINDOW_UPDATE: u8 = 0x8;
const FRAME_CONTINUATION: u8 = 0x9;
const FRAME_PRIORITY_UPDATE: u8 = 0x10;
const FRAME_DATA: u8 = 0x0;
const FRAME_HEADERS: u8 = 0x1;

const FLAG_END_HEADERS: u8 = 0x4;
const FLAG_SETTINGS_ACK: u8 = 0x1;
const FLAG_PADDED: u8 = 0x8;
const FLAG_PRIORITY: u8 = 0x20;

struct HeaderFrame {
    remaining: usize,
    prefix_remaining: usize,
    padded: bool,
    pad_length: Option<usize>,
    header_remaining: Option<usize>,
    end_headers: bool,
}

impl HeaderFrame {
    fn new(kind: u8, flags: u8, length: usize) -> Self {
        let headers = kind == FRAME_HEADERS;
        let padded = headers && flags & FLAG_PADDED != 0;
        let priority = headers && flags & FLAG_PRIORITY != 0;
        let prefix_remaining = usize::from(padded) + usize::from(priority) * 5;
        Self {
            remaining: length,
            prefix_remaining,
            padded,
            pad_length: (!padded).then_some(0),
            header_remaining: (prefix_remaining == 0).then_some(length),
            end_headers: kind == FRAME_HEADERS && flags & FLAG_END_HEADERS != 0
                || kind == FRAME_CONTINUATION && flags & FLAG_END_HEADERS != 0,
        }
    }
}

pub(super) struct FrameValidatedStream<S> {
    inner: S,
    preface_seen: usize,
    header: [u8; FRAME_HEADER_LEN],
    header_len: usize,
    payload_remaining: usize,
    ready: Vec<u8>,
    ready_position: usize,
    last_stream_id: u32,
    fatal: Option<h2::Reason>,
    goaway: Option<Vec<u8>>,
    goaway_position: usize,
    goaway_flushed: bool,
    eof: bool,
    hpack: HeaderBlockValidator,
    header_frame: Option<HeaderFrame>,
    header_stream_id: Option<u32>,
}

impl<S: Unpin> FrameValidatedStream<S> {
    pub(super) fn new(inner: S, max_header_field_size: usize) -> Self {
        Self {
            inner,
            preface_seen: 0,
            header: [0; FRAME_HEADER_LEN],
            header_len: 0,
            payload_remaining: 0,
            ready: Vec::with_capacity(8192),
            ready_position: 0,
            last_stream_id: 0,
            fatal: None,
            goaway: None,
            goaway_position: 0,
            goaway_flushed: false,
            eof: false,
            hpack: HeaderBlockValidator::new(max_header_field_size),
            header_frame: None,
            header_stream_id: None,
        }
    }

    fn process(&mut self, mut bytes: &[u8]) {
        if self.preface_seen < HTTP2_PREFACE_LEN {
            let length = (HTTP2_PREFACE_LEN - self.preface_seen).min(bytes.len());
            self.ready.extend_from_slice(&bytes[..length]);
            self.preface_seen += length;
            bytes = &bytes[length..];
        }

        while !bytes.is_empty() && self.fatal.is_none() {
            if self.payload_remaining != 0 {
                let length = self.payload_remaining.min(bytes.len());
                if !self.inspect_payload(&bytes[..length]) {
                    self.begin_goaway(h2::Reason::ENHANCE_YOUR_CALM);
                    break;
                }
                self.ready.extend_from_slice(&bytes[..length]);
                self.payload_remaining -= length;
                bytes = &bytes[length..];
                continue;
            }

            let length = (FRAME_HEADER_LEN - self.header_len).min(bytes.len());
            self.header[self.header_len..self.header_len + length]
                .copy_from_slice(&bytes[..length]);
            self.header_len += length;
            bytes = &bytes[length..];
            if self.header_len != FRAME_HEADER_LEN {
                break;
            }

            let frame_length = frame_length(&self.header);
            let kind = self.header[3];
            let flags = self.header[4];
            let stream_id = frame_stream_id(&self.header);
            if let Some(reason) = invalid_frame_shape(kind, flags, frame_length) {
                self.begin_goaway(reason);
                break;
            }

            self.last_stream_id = self.last_stream_id.max(stream_id);
            self.ready.extend_from_slice(&self.header);
            self.header_len = 0;
            self.payload_remaining = frame_length;
            self.start_frame_payload(kind, flags, stream_id, frame_length);
        }
    }

    fn start_frame_payload(&mut self, kind: u8, flags: u8, stream_id: u32, length: usize) {
        self.header_frame = None;
        match kind {
            FRAME_HEADERS => {
                self.hpack.reset();
                self.header_stream_id = Some(stream_id);
                self.header_frame = Some(HeaderFrame::new(kind, flags, length));
            }
            FRAME_CONTINUATION if self.header_stream_id == Some(stream_id) => {
                self.header_frame = Some(HeaderFrame::new(kind, flags, length));
            }
            _ => {}
        }
        if length == 0 {
            self.finish_header_frame();
        }
    }

    fn inspect_payload(&mut self, mut bytes: &[u8]) -> bool {
        let Some(mut frame) = self.header_frame.take() else {
            return true;
        };
        while !bytes.is_empty() && frame.remaining != 0 {
            if frame.prefix_remaining != 0 {
                let length = frame.prefix_remaining.min(bytes.len());
                if frame.padded && frame.pad_length.is_none() {
                    frame.pad_length = Some(usize::from(bytes[0]));
                }
                frame.prefix_remaining -= length;
                frame.remaining -= length;
                bytes = &bytes[length..];
                if frame.prefix_remaining == 0 {
                    frame.header_remaining =
                        frame.remaining.checked_sub(frame.pad_length.unwrap_or(0));
                }
                continue;
            }

            let Some(header_remaining) = frame.header_remaining else {
                frame.remaining = 0;
                break;
            };
            if header_remaining != 0 {
                let length = header_remaining.min(bytes.len());
                if self.hpack.validate(&bytes[..length]).is_err() {
                    return false;
                }
                frame.header_remaining = Some(header_remaining - length);
                frame.remaining -= length;
                bytes = &bytes[length..];
                continue;
            }

            let length = frame.remaining.min(bytes.len());
            frame.remaining -= length;
            bytes = &bytes[length..];
        }

        if frame.remaining == 0 {
            if frame.end_headers {
                self.hpack.reset();
                self.header_stream_id = None;
            }
        } else {
            self.header_frame = Some(frame);
        }
        true
    }

    fn finish_header_frame(&mut self) {
        if self
            .header_frame
            .as_ref()
            .is_some_and(|frame| frame.remaining == 0)
        {
            let end_headers = self
                .header_frame
                .as_ref()
                .is_some_and(|frame| frame.end_headers);
            self.header_frame = None;
            if end_headers {
                self.hpack.reset();
                self.header_stream_id = None;
            }
        }
    }

    fn begin_goaway(&mut self, reason: h2::Reason) {
        self.fatal = Some(reason);
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + 8);
        frame.extend_from_slice(&[0, 0, 8, FRAME_GOAWAY, 0, 0, 0, 0, 0]);
        frame.extend_from_slice(&(self.last_stream_id & 0x7fff_ffff).to_be_bytes());
        frame.extend_from_slice(&u32::from(reason).to_be_bytes());
        self.goaway = Some(frame);
    }

    fn read_ready(&mut self, buffer: &mut ReadBuf<'_>) {
        let length = (self.ready.len() - self.ready_position).min(buffer.remaining());
        buffer.put_slice(&self.ready[self.ready_position..self.ready_position + length]);
        self.ready_position += length;
        if self.ready_position == self.ready.len() {
            self.ready.clear();
            self.ready_position = 0;
        }
    }
}

impl<S: AsyncWrite + Unpin> FrameValidatedStream<S> {
    fn poll_goaway(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.goaway_flushed {
            return Poll::Ready(Ok(()));
        }
        let Some(goaway) = self.goaway.as_ref() else {
            self.goaway_flushed = true;
            return Poll::Ready(Ok(()));
        };
        while self.goaway_position < goaway.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &goaway[self.goaway_position..]) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write HTTP/2 GOAWAY",
                    )));
                }
                Poll::Ready(Ok(written)) => self.goaway_position += written,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }
        match Pin::new(&mut self.inner).poll_flush(cx) {
            Poll::Ready(Ok(())) => {
                self.goaway_flushed = true;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for FrameValidatedStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.ready_position < this.ready.len() {
                this.read_ready(buffer);
                return Poll::Ready(Ok(()));
            }
            if this.fatal.is_some() {
                match this.poll_goaway(cx) {
                    Poll::Ready(Ok(())) => return Poll::Ready(Ok(())),
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Pending => return Poll::Pending,
                }
            }
            if this.eof {
                return Poll::Ready(Ok(()));
            }

            let mut input = [0_u8; 8192];
            let mut read = ReadBuf::new(&mut input);
            match Pin::new(&mut this.inner).poll_read(cx, &mut read) {
                Poll::Ready(Ok(())) if read.filled().is_empty() => this.eof = true,
                Poll::Ready(Ok(())) => {
                    this.process(read.filled());
                }
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FrameValidatedStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.fatal.is_some() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "HTTP/2 connection rejected before write",
            )));
        }
        Pin::new(&mut this.inner).poll_write(cx, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

fn frame_length(header: &[u8; FRAME_HEADER_LEN]) -> usize {
    ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize
}

fn frame_stream_id(header: &[u8; FRAME_HEADER_LEN]) -> u32 {
    u32::from_be_bytes(header[5..9].try_into().expect("frame stream id")) & 0x7fff_ffff
}

fn invalid_frame_shape(kind: u8, flags: u8, length: usize) -> Option<h2::Reason> {
    if length > DEFAULT_MAX_FRAME_SIZE {
        return Some(h2::Reason::FRAME_SIZE_ERROR);
    }
    let fixed_length = match kind {
        FRAME_DATA => flags & FLAG_PADDED != 0 && length == 0,
        FRAME_HEADERS => {
            let overhead =
                usize::from(flags & FLAG_PADDED != 0) + usize::from(flags & FLAG_PRIORITY != 0) * 5;
            length <= overhead
        }
        FRAME_PRIORITY => length != 5,
        FRAME_RST_STREAM => length != 4,
        FRAME_SETTINGS => {
            !length.is_multiple_of(6) || (flags & FLAG_SETTINGS_ACK != 0 && length != 0)
        }
        FRAME_PING => length != 8,
        FRAME_GOAWAY => length < 8,
        FRAME_WINDOW_UPDATE => length != 4,
        FRAME_PRIORITY_UPDATE => length < 4,
        _ => false,
    };
    fixed_length.then_some(h2::Reason::FRAME_SIZE_ERROR)
}
