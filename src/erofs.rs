//! EROFS image builds through a pinned mkfs.erofs fed a streamed tar

use crate::error::Error;
use crate::settings::erofs::{ARGS, PIPE_CAP, STDERR_TAIL, VERSION};
use crate::writer::{WriteEntry, Writer};
use std::io::SeekFrom;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, BufWriter};
use tokio::process::{Child, ChildStdin, Command};
use tokio::task::JoinHandle;

/// A pinned mkfs.erofs and the scratch directory its images and temporaries live in
///
/// Images are unlinked scratch files, so a crash or a dropped build leaves no file behind.
#[derive(Debug, Clone)]
pub struct Erofs {
    program: PathBuf,
    scratch: PathBuf,
}

impl Erofs {
    /// Check that `program` is mkfs.erofs 1.9.4 and bind it to `scratch`
    pub async fn new(
        program: impl Into<PathBuf>,
        scratch: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let program = program.into();
        let output = Command::new(&program)
            .arg("-V")
            .env_clear()
            .output()
            .await?;
        let banner = String::from_utf8_lossy(&output.stdout);
        let first = banner.lines().next().unwrap_or_default();
        if !output.status.success() || !first.contains(VERSION) {
            return Err(Error::ErofsVersion(first.to_owned()));
        }
        let scratch = scratch.into();
        Ok(Self { program, scratch })
    }

    /// Start one image build; dropping the build kills mkfs.erofs and frees its image
    pub fn build(&self) -> Result<ErofsBuild, Error> {
        let image = tempfile::tempfile_in(&self.scratch)?;
        let output = format!("/proc/{}/fd/{}", std::process::id(), image.as_raw_fd());
        let mut child = Command::new(&self.program)
            .args(ARGS)
            .arg(output)
            .env_clear()
            .env("TMPDIR", &self.scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (stdin, stderr) = child
            .stdin
            .take()
            .zip(child.stderr.take())
            .ok_or(Error::Aborted)?;
        Ok(ErofsBuild {
            tar: Writer::tar(BufWriter::with_capacity(PIPE_CAP, stdin)),
            stderr: Some(tokio::spawn(tail(stderr))),
            image: tokio::fs::File::from_std(image),
            child,
        })
    }
}

/// One running image build fed entries in archive order
pub struct ErofsBuild {
    child: Child,
    tar: Writer<BufWriter<ChildStdin>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    image: tokio::fs::File,
}

impl ErofsBuild {
    /// Stream one entry into the image
    pub async fn append<B>(&mut self, entry: WriteEntry<B>) -> Result<(), Error>
    where
        B: AsyncRead + Unpin + Send,
    {
        match self.tar.append(entry).await {
            Err(Error::Io(io)) if io.kind() == std::io::ErrorKind::BrokenPipe => {
                exit(&mut self.child, self.stderr.take()).await?;
                Err(Error::Io(io))
            }
            result => result,
        }
    }

    /// End the tar, wait for mkfs.erofs, and return the image rewound to its start
    pub async fn finish(self) -> Result<tokio::fs::File, Error> {
        let Self {
            mut child,
            tar,
            stderr,
            mut image,
        } = self;
        let closed = tar.finish().await.map(drop);
        exit(&mut child, stderr).await?;
        closed?;
        image.seek(SeekFrom::Start(0)).await?;
        Ok(image)
    }
}

/// Wait for mkfs.erofs to exit, failing with its stderr tail unless it succeeded
async fn exit(child: &mut Child, stderr: Option<JoinHandle<Vec<u8>>>) -> Result<(), Error> {
    let status = child.wait().await?;
    let stderr = match stderr {
        Some(handle) => handle.await.unwrap_or_default(),
        None => Vec::new(),
    };
    match status.success() {
        true => Ok(()),
        false => Err(failed(status, &stderr)),
    }
}

/// The typed failure of an exited mkfs.erofs
fn failed(status: ExitStatus, stderr: &[u8]) -> Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    Error::Erofs { status, stderr }
}

/// Drain a child's stderr, keeping its last bytes
async fn tail(mut stderr: impl AsyncRead + Unpin) -> Vec<u8> {
    let (mut kept, mut chunk) = (Vec::new(), [0u8; 1024]);
    while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
        kept.extend_from_slice(&chunk[..read]);
        let over = kept.len().saturating_sub(STDERR_TAIL);
        kept.drain(..over);
    }
    kept
}
