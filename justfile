set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

check:
    bash .github/scripts/check-agent-layout.sh
    cargo fmt --all -- --check
    cargo check --workspace --all-targets --locked
    cargo clippy -p etar --all-targets --locked -- -D warnings
    cargo clippy -p etar --all-targets --features erofs --locked -- -D warnings

fmt:
    cargo fmt --all

test:
    cargo nextest run -p etar --locked

test-ci:
    cargo nextest run --profile ci -p etar --locked

erofs-tools:
    bash scripts/erofs-tools.sh

test-erofs: erofs-tools
    cargo nextest run -p etar --features erofs --test erofs --locked

test-doc:
    cargo test -p etar --doc --locked

doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc -p etar --no-deps --all-features --locked

docs-open:
    RUSTDOCFLAGS="-D warnings" cargo doc -p etar --no-deps --all-features --locked --open

msrv:
    cargo +1.96 check -p etar --all-targets --locked

package-check:
    cargo package -p etar --locked --allow-dirty
    cargo package -p etar --locked --allow-dirty --list

audit:
    cargo deny check

bench-build mode="simulation":
    RUSTFLAGS="" cargo codspeed build -m {{mode}} -p etar --features erofs

bench-run mode="simulation": erofs-tools
    RUSTFLAGS="" cargo codspeed run -m {{mode}} -p etar

bench:
    just bench-build
    just bench-run

semver-check baseline="" release_type="minor":
    bash scripts/semver-check.sh "{{baseline}}" "{{release_type}}"

release-plan:
    bash scripts/release.sh --dry-run

release-test:
    bash tests/release_plan.sh

size:
    bash scripts/pr-size-gate.sh

ci:
    just check
    just test-ci
    just test-doc
    just doc-check
    just package-check
    just release-test
    just audit
    just msrv
