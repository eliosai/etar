//! EROFS image builds through the embedded mkfs.erofs fed a streamed tar

use crate::error::Error;
use crate::settings::erofs::{ARGS, PIPE_CAP, STDERR_TAIL};
use crate::writer::{WriteEntry, Writer};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::fs::File;
use std::io::{SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, BufWriter};
use tokio::process::{Child, ChildStdin, Command};
use tokio::task::JoinHandle;

/// The embedded mkfs.erofs and the scratch directory its images and temporaries live in
///
/// The tool is erofs-utils 1.9.4 with the patches in `erofs/`, built by this crate and run from a sealed memfd.
/// Images are unlinked scratch files, so a crash or a dropped build leaves no file behind.
#[derive(Debug, Clone)]
pub struct Erofs {
    program: PathBuf,
    scratch: PathBuf,
}

impl Erofs {
    /// The mkfs.erofs this crate built from its vendored erofs-utils
    const MKFS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mkfs.erofs"));

    /// Bind the embedded mkfs.erofs to `scratch`, loading it into this process on first use; builds need `/proc`
    pub fn new(scratch: impl Into<PathBuf>) -> Result<Self, Error> {
        static TOOL: OnceLock<File> = OnceLock::new();
        let tool = match TOOL.get() {
            Some(tool) => tool,
            None => {
                let sealed = Self::sealed()?;
                TOOL.get_or_init(|| sealed)
            }
        };
        let program = PathBuf::from(format!("/proc/self/fd/{}", tool.as_raw_fd()));
        let scratch = scratch.into();
        Ok(Self { program, scratch })
    }

    /// An executable memfd holding mkfs.erofs, sealed and reopened read-only so the kernel executes it
    fn sealed() -> Result<File, Error> {
        let flags = MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING;
        let memfd = match memfd_create("mkfs.erofs", flags | MemfdFlags::EXEC) {
            Err(rustix::io::Errno::INVAL) => memfd_create("mkfs.erofs", flags),
            made => made,
        };
        let mut file = File::from(memfd.map_err(std::io::Error::from)?);
        file.write_all(Self::MKFS)?;
        let seals = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
        fcntl_add_seals(&file, seals).map_err(std::io::Error::from)?;
        Ok(File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))?)
    }

    /// Start one image build; dropping the build kills mkfs.erofs and frees its image
    pub async fn build(&self) -> Result<ErofsBuild, Error> {
        let image = tempfile::tempfile_in(&self.scratch)?;
        let output = format!("/proc/{}/fd/{}", std::process::id(), image.as_raw_fd());
        let mut child = Command::new(&self.program)
            .arg0("mkfs.erofs")
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
    // Fields drop in order, so mkfs.erofs dies before the image closes
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
