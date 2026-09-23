//! Compression sniffing and transparent decode for opened archives

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Bytes peeked to detect the compression magic
const PEEK_LEN: usize = 4;
/// zstd frame magic
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// Buffered-reader capacity feeding the decoder
const BUF_CAP: usize = 64 * 1024;

/// Wrap a reader in the decoder its magic selects, else pass it through
pub async fn decoded<R>(src: R) -> std::io::Result<Box<dyn AsyncRead + Unpin + Send>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buf = BufReader::with_capacity(BUF_CAP, src);
    let magic = peek(&mut buf).await?;
    Ok(wrap(&magic, buf))
}

/// Copy the leading magic bytes without consuming them
async fn peek<R>(buf: &mut BufReader<R>) -> std::io::Result<[u8; PEEK_LEN]>
where
    R: AsyncRead + Unpin + Send,
{
    let head = buf.fill_buf().await?;
    let seen = head.len().min(PEEK_LEN);
    let mut magic = [0u8; PEEK_LEN];
    magic[..seen].copy_from_slice(&head[..seen]);
    Ok(magic)
}

/// Select the decoder for the magic, wrapping the still-buffered reader
fn wrap<R>(magic: &[u8], buf: BufReader<R>) -> Box<dyn AsyncRead + Unpin + Send>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    use async_compression::tokio::bufread::ZstdDecoder;
    if magic.starts_with(&ZSTD_MAGIC) {
        Box::new(ZstdDecoder::new(buf))
    } else {
        Box::new(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_compression::tokio::write::ZstdEncoder;
    use pretty_assertions::assert_eq;
    use std::io::Cursor;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_all(bytes: Vec<u8>) -> Vec<u8> {
        let mut reader = decoded(Cursor::new(bytes)).await.unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn test_zstd_roundtrips() {
        let mut enc = ZstdEncoder::new(Vec::new());
        enc.write_all(b"hello tara").await.unwrap();
        enc.shutdown().await.unwrap();
        assert_eq!(read_all(enc.into_inner()).await, b"hello tara");
    }

    #[tokio::test]
    async fn test_plain_passes_through() {
        assert_eq!(read_all(b"plain bytes".to_vec()).await, b"plain bytes");
    }

    #[tokio::test]
    async fn test_empty_input_is_empty() {
        assert_eq!(read_all(Vec::new()).await, b"");
    }
}
