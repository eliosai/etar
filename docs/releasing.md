# Releasing

A release is the pull request `just release <version>` prepares. Its merge publishes the version.

1. From an up-to-date `main`, run `just release 0.2.0`. It writes the version into `Cargo.toml` and `Cargo.lock`.
2. Commit `chore: release v0.2.0` on the branch `release/v0.2.0` and open its pull request.
3. CI treats any pull request carrying a version no tag names as a release: it checks the branch names the version, the version follows the newest tag, and `cargo-semver-checks` finds no API break that version may not carry. Before 1.0, a minor bump may break the API.
4. Rebase-merge once `gate` passes. The `Release` workflow sees a version on `main` no tag names, publishes it to crates.io and creates the GitHub release with tag `v0.2.0`.

Never run `cargo publish`, push a version tag or create a GitHub release by hand. A tag never moves. If a release stops half way, run the `Release` workflow again from `main`: a version crates.io already holds is skipped, and the tag and release land on the commit which set the version.

## Publishing access

crates.io trusts the `eliosai/etar` repository, the `release.yml` workflow and the `crates-io` GitHub environment. The workflow exchanges GitHub's OIDC identity for a short-lived crates.io token, so GitHub holds no crates.io token. The `crates-io` environment deploys only from `main`.
