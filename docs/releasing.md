# Releasing

The `release.yml` workflow is the only publisher. Do not run `cargo publish`,
create a version tag, or create a GitHub release from a workstation. Every
release starts from `main`, runs the Rust checks, and uses the version selected
by `scripts/release.sh`. Run `just release-plan` to inspect that version.

## First release

crates.io cannot attach a trusted publisher to `etar` until the crate exists.
Create a short-lived crates.io token authorized to publish a new crate and put
it in the `ETAR_BOOTSTRAP_CARGO_TOKEN` secret of the protected `crates-io`
GitHub environment. Put the write-enabled SSH deploy key in that environment
as `RELEASE_DEPLOY_KEY`. The `main` and `release tags` rulesets permit that key
to push the release commit and version tag.

Once the code and CI are green, set the repository variable
`ETAR_RELEASE_ENABLED=true`. The workflow will publish `v0.1.0` from the
version in `Cargo.toml`, then create the GitHub release. After it succeeds,
revoke the bootstrap token and delete its GitHub secret.

## Later releases

Configure [crates.io trusted publishing](https://github.com/rust-lang/rfcs/blob/master/text/3691-trusted-publishing-cratesio.md#trusted-publisher-configuration-on-cratesio)
for `eliosai/etar`, workflow `release.yml`, and environment `crates-io`
before the next release. The workflow then exchanges GitHub's OIDC identity
for a short-lived crates.io token. Keep `ETAR_RELEASE_ENABLED` unset while
changing publisher credentials.

`feat:` advances the minor version. `fix:`, `perf:`, and `refactor:` advance
the patch. A `!` subject, `BREAKING CHANGE:` body, or API break found by
`cargo-semver-checks` advances the major; before 1.0, that advances the minor
component. Docs, test, and CI commits leave the version unchanged.

The PR `semver` job checks the API against the PR base. Add the `semver-major`
label for an intentional break. The first PR that introduces `etar` needs API
review because its base commit contains `tara` instead. Use `release-preview`
to check the version and package without publishing. Use workflow dispatch to
retry a release interrupted after the version tag was pushed.
