# etar

`etar` reads, writes, and validates raw tar and tar.zst over Tokio byte
streams. The library never selects an object store, hashes content, or
materializes archive paths. `object_store` is a dev dependency for the
integration test, not part of the library's dependency graph.

## Layout and interface

- `src/entry.rs` owns `EntryPath`, entry kinds and Unix metadata
- `src/reader.rs` owns sequential input, completion, and validation
- `src/guard.rs` caps extension records before the parser buffers them
- `src/open.rs` owns bounded body and byte meters
- `src/writer.rs` owns output format and finalization
- `src/pack.rs` encodes one tar entry
- `tests/` exercises only the public crate interface; `fuzz/` owns AFL targets
- `benches/` owns CodSpeed Divan benchmarks

The public interface lives at the crate root: `Reader`, `Writer`, `validate`,
and their checked entry vocabulary. Add a public item only when those
operations cannot express a caller's behavior. Keep implementation modules
private and re-export public items only from `lib.rs`.

An opened entry path is validated, but a symlink or hardlink target remains
untrusted data. Callers choose whether and how to materialize links. The crate
does not extract a tar into a filesystem. A read is complete only after
`next_entry` returns `Ok(None)`; writers must call `finish` and discard sinks
after any failure.

## Work and review

Write one red test through the smallest honest interface, make it pass, then
refactor. Unit tests stay beside their logic. Integration tests and crate-local
fixtures live in `tests/`.
Use `cargo nextest` for non-documentation tests and `cargo test --doc` for
examples. Keep AFL targets bounded and deterministic.

All dependencies are declared in `[workspace.dependencies]` and inherited by
members. Benchmark imports use the `divan` alias for
`codspeed-divan-compat`. Do not add regular `divan` or Criterion.

Measure before optimizing. Use local CodSpeed Divan runs, compare simulation
and walltime, and keep a change only when correctness holds and the measured
gain justifies the complexity. CI uses Blacksmith, not CodSpeed macro runners.

Production paths return typed errors and never use `unwrap`, `expect` or
`panic`. Keep functions short, split files by behavior, and keep comments
specific. Public rustdoc and README claims must match tested behavior.

`main` is the working branch. Do not create another branch. Run the local
checks before pushing when Josh asks. Never publish manually: the release
workflow owns versions, tags, crates.io publication, and GitHub releases.

## Agent files

The skill packages live in `.agents/skills/`. `CLAUDE.md` points to this file
and `.claude/skills` points to `.agents/skills/`. Read the matching skill before
working in its domain: `tdd` and `rust-testing` for behavior, `rust-best-practices`
and `rust-async-patterns` for Rust, `codebase-design` for interface changes,
`codspeed-setup-harness` and `codspeed-optimize` for performance, and
`stop-slop` and `josh-voice` for prose. Use `code-review` only after a fixed
commit baseline and spec source exist; review uncommitted work against the
working tree. Do not edit a copied skill as part of product work.
