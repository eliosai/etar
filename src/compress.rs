//! Compression sniffing and transparent decode for opened archives

use crate::settings::compression::{BUF_CAP, PEEK_LEN, ZSTD_MAGIC};
use std::io::Cursor;
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};

/// Wrap a reader in the decoder its magic selects, else pass it through
pub async fn decoded<R>(mut src: R) -> std::io::Result<Box<dyn AsyncRead + Unpin + Send>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut magic = [0u8; PEEK_LEN];
    let mut seen = 0;
    while seen < PEEK_LEN {
        let read = src.read(&mut magic[seen..]).await?;
        if read == 0 {
            break;
        }
        seen += read;
    }
    let replay = Cursor::new(magic[..seen].to_vec()).chain(src);
    let buf = BufReader::with_capacity(BUF_CAP, replay);
    Ok(wrap(&magic[..seen], buf))
}

/// Select the decoder for the magic, wrapping the still-buffered reader
fn wrap<R>(magic: &[u8], buf: BufReader<R>) -> Box<dyn AsyncRead + Unpin + Send>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    use async_compression::tokio::bufread::ZstdDecoder;
    if magic.starts_with(&ZSTD_MAGIC) {
        let mut decoder = ZstdDecoder::new(buf);
        decoder.multiple_members(true);
        Box::new(decoder)
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
        enc.write_all(b"hello etar").await.unwrap();
        enc.shutdown().await.unwrap();
        assert_eq!(read_all(enc.into_inner()).await, b"hello etar");
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
