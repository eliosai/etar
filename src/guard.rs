//! Bound extension records before the tar parser buffers them

use crate::error::Error;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_tar::Header;

/// Inspects raw tar headers without copying file payloads
pub struct HeaderGuard<R> {
    inner: R,
    header: [u8; 512],
    filled: usize,
    emitted: usize,
    data_remaining: u64,
    next_data: u64,
    next_trailer: bool,
    trailer: bool,
    max_metadata_bytes: u64,
    pax_body: Option<Vec<u8>>,
    pax_remaining: u64,
    pax_size: Option<u64>,
}

impl<R> HeaderGuard<R> {
    /// Create a guard around a decoded tar source
    pub fn new(inner: R, max_metadata_bytes: u64) -> Self {
        Self {
            inner,
            header: [0; 512],
            filled: 0,
            emitted: 0,
            data_remaining: 0,
            next_data: 0,
            next_trailer: false,
            trailer: false,
            max_metadata_bytes,
            pax_body: None,
            pax_remaining: 0,
            pax_size: None,
        }
    }

    fn inspect(&mut self) -> io::Result<()> {
        if self.header.iter().all(|byte| *byte == 0) {
            self.next_trailer = true;
            return Ok(());
        }
        let kind = self.header[156];
        if !matches!(
            kind,
            0 | b'0' | b'1' | b'2' | b'5' | b'7' | b'x' | b'L' | b'K'
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                Error::DisallowedEntryType,
            ));
        }
        let mut header = Header::new_old();
        header.as_mut_bytes().copy_from_slice(&self.header);
        let raw_size = header.size().map_err(|_| malformed())?;
        if matches!(kind, b'x' | b'L' | b'K') && raw_size > self.max_metadata_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                Error::LimitExceeded("max_metadata_bytes"),
            ));
        }
        if kind == b'x' {
            self.pax_body = Some(Vec::new());
            self.pax_remaining = raw_size;
        }
        let size = if matches!(kind, b'x' | b'L' | b'K') {
            raw_size
        } else {
            self.pax_size.take().unwrap_or(raw_size)
        };
        self.next_data = size
            .checked_add(511)
            .map(|rounded| rounded / 512 * 512)
            .ok_or_else(malformed)?;
        Ok(())
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for HeaderGuard<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.trailer {
            return Pin::new(&mut this.inner).poll_read(cx, out);
        }
        if this.data_remaining > 0 {
            return this.read_data(cx, out);
        }
        while this.filled < 512 {
            let mut read = ReadBuf::new(&mut this.header[this.filled..]);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut read))?;
            let count = read.filled().len();
            if count == 0 {
                return Poll::Ready(Ok(()));
            }
            this.filled += count;
        }
        if this.emitted == 0 {
            this.inspect()?;
        }
        let count = out.remaining().min(512 - this.emitted);
        out.put_slice(&this.header[this.emitted..this.emitted + count]);
        this.emitted += count;
        if this.emitted == 512 {
            this.filled = 0;
            this.emitted = 0;
            this.data_remaining = this.next_data;
            this.next_data = 0;
            this.trailer = this.next_trailer;
            if this.data_remaining == 0
                && let Some(body) = this.pax_body.take()
            {
                this.pax_size = parse_pax_size(&body)?;
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncRead + Unpin> HeaderGuard<R> {
    fn read_data(&mut self, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let cap = usize::try_from(self.data_remaining).unwrap_or(usize::MAX);
        let count = {
            let mut limited = out.take(cap);
            ready!(Pin::new(&mut self.inner).poll_read(cx, &mut limited))?;
            let bytes = limited.filled();
            if let Some(body) = &mut self.pax_body {
                let cap = usize::try_from(self.pax_remaining).unwrap_or(usize::MAX);
                let keep = bytes.len().min(cap);
                body.extend_from_slice(&bytes[..keep]);
                self.pax_remaining -= keep as u64;
            }
            bytes.len()
        };
        // SAFETY: the nested ReadBuf initialized exactly `count` bytes in `out`'s unfilled region
        unsafe { out.assume_init(count) };
        out.advance(count);
        self.data_remaining -= count as u64;
        if self.data_remaining == 0
            && let Some(body) = self.pax_body.take()
        {
            self.pax_size = parse_pax_size(&body)?;
        }
        Poll::Ready(Ok(()))
    }
}

fn parse_pax_size(mut body: &[u8]) -> io::Result<Option<u64>> {
    let mut size = None;
    while !body.is_empty() {
        let space = body
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(malformed)?;
        let length = decimal(&body[..space]).ok_or_else(malformed)?;
        let length = usize::try_from(length).map_err(|_| malformed())?;
        if length <= space + 2 || length > body.len() {
            return Err(malformed());
        }
        let record = &body[space + 1..length];
        if !record.ends_with(b"\n") {
            return Err(malformed());
        }
        if let Some(value) = record.strip_prefix(b"size=") {
            size = Some(decimal(&value[..value.len() - 1]).ok_or_else(malformed)?);
        }
        body = &body[length..];
    }
    Ok(size)
}

fn decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0u64, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value.checked_mul(10)?.checked_add(u64::from(byte - b'0')))
            .flatten()
    })
}

fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Error::Malformed)
}
