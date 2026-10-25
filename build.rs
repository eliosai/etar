//! Builds the vendored, patched erofs-utils into mkfs.erofs and fsck.erofs when the `erofs` feature is on

use std::env;
use std::path::{Path, PathBuf};

/// The liberofs sources the configured build compiles
const LIB: [&str; 42] = [
    "backends/nbd.c",
    "base64.c",
    "bitops.c",
    "blobchunk.c",
    "block_list.c",
    "cache.c",
    "compress.c",
    "compress_hints.c",
    "compressor.c",
    "compressor_deflate.c",
    "compressor_libzstd.c",
    "config.c",
    "data.c",
    "decompress.c",
    "dedupe.c",
    "dedupe_ext.c",
    "dir.c",
    "diskbuf.c",
    "exclude.c",
    "fragments.c",
    "global.c",
    "gzran.c",
    "hashmap.c",
    "importer.c",
    "inode.c",
    "io.c",
    "kite_deflate.c",
    "metabox.c",
    "namei.c",
    "rebuild.c",
    "remotes/docker_config.c",
    "remotes/oci.c",
    "sha256.c",
    "super.c",
    "tar.c",
    "uuid.c",
    "uuid_unparse.c",
    "vmdk.c",
    "workqueue.c",
    "xattr.c",
    "xxhash.c",
    "zmap.c",
];

/// The tools linked from liberofs, each from its own `main.c`
const TOOLS: [&str; 2] = ["mkfs", "fsck"];

/// The compiler settings cc reads, which cargo_metadata(false) leaves unwatched
const COMPILER_ENV: [&str; 6] = [
    "CC",
    "CFLAGS",
    "TARGET_CC",
    "TARGET_CFLAGS",
    "HOST_CC",
    "CRATE_CC_NO_DEFAULTS",
];

fn main() {
    println!("cargo:rerun-if-changed=erofs");
    for name in COMPILER_ENV {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if env::var_os("CARGO_FEATURE_EROFS").is_none() {
        return;
    }
    let target = (
        env::var("CARGO_CFG_TARGET_OS"),
        env::var("CARGO_CFG_TARGET_ENV"),
    );
    if target.0.as_deref() != Ok("linux") || target.1.as_deref() != Ok("gnu") {
        panic!(
            "the erofs feature builds mkfs.erofs for linux-gnu only, the target erofs/config.h describes"
        );
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let zstd = PathBuf::from(env::var_os("DEP_ZSTD_ROOT").expect(
        "the erofs feature links the static zstd zstd-sys bundles, not a pkg-config or cmake one",
    ));
    let erofs = Path::new(env!("CARGO_MANIFEST_DIR")).join("erofs");
    let lib = objects(&erofs, &zstd, LIB.iter().map(|file| format!("lib/{file}")));
    for tool in TOOLS {
        let main = objects(&erofs, &zstd, [format!("{tool}/main.c")]);
        link(&out.join(format!("{tool}.erofs")), &main, &lib, &zstd);
    }
    println!(
        "cargo:rustc-env=ETAR_FSCK_EROFS={}",
        out.join("fsck.erofs").display()
    );
}

/// Compile `sources` under the vendored tree with the configured defines, keeping the objects unlinked
fn objects(erofs: &Path, zstd: &Path, sources: impl IntoIterator<Item = String>) -> Vec<PathBuf> {
    let upstream = erofs.join("upstream");
    cc::Build::new()
        .files(sources.into_iter().map(|source| upstream.join(source)))
        .include(erofs)
        .include(upstream.join("include"))
        .include(upstream.join("lib"))
        .include(zstd.join("include"))
        .define("HAVE_CONFIG_H", None)
        .define("NDEBUG", None)
        .define("_LARGEFILE_SOURCE", None)
        .define("_FILE_OFFSET_BITS", "64")
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .cargo_metadata(false)
        .compile_intermediates()
}

/// Link one stripped executable at `binary` from its `main` objects, liberofs, the static zstd, and pthreads
fn link(binary: &Path, main: &[PathBuf], lib: &[PathBuf], zstd: &Path) {
    let mut command = cc::Build::new()
        .cargo_metadata(false)
        .get_compiler()
        .to_command();
    let status = command
        .args(main)
        .args(lib)
        .arg(zstd.join("libzstd.a"))
        .args(["-lpthread", "-s", "-Wl,--gc-sections", "-o"])
        .arg(binary)
        .status()
        .expect("the C compiler runs");
    assert!(status.success(), "linking {} failed", binary.display());
}
