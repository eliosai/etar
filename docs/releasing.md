# Releasing

The `release.yml` workflow is the only publisher. Do not run `cargo publish`,
create a version tag, or create a GitHub release from a workstation. Every
release starts from `main`, runs the Rust checks, and uses the version selected
by `scripts/release.sh`. Run `just release-plan` to inspect that version.

## Publishing access

crates.io trusts the `eliosai/etar` repository, the `release.yml` workflow,
and the `crates-io` GitHub environment. The workflow exchanges GitHub's OIDC
identity for a short-lived crates.io token. The GitHub environment accepts
protected branches without a deployment reviewer, so the release job needs no
deployment approval. Keep `ETAR_RELEASE_ENABLED=true` for CD and set it to `false`
when you need to pause releases.

GitHub stores the write-enabled SSH deploy key as `RELEASE_DEPLOY_KEY` in the
`crates-io` environment. The `main` and `release tags` rulesets permit that
key to push the release commit and version tag. GitHub holds no crates.io
bootstrap token.

## Versions

`feat:` advances the minor version. `fix:`, `perf:`, and `refactor:` advance
the patch. A `!` subject, `BREAKING CHANGE:` body, or API break found by
`cargo-semver-checks` advances the major; before 1.0, that advances the minor
component. Docs, test, and CI commits leave the version unchanged.

The PR `semver` job checks the API against the PR base. Add the `semver-major`
label for an intentional break. Run `release-preview` to check the version and
package without publishing. If a release stops after it pushes the version
tag, dispatch `release.yml` again from `main` to complete it.
