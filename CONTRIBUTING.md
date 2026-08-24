# Contributing to Offline Protocol SDK

Thank you for your interest in contributing! This document provides guidelines for contributing to the project.

## Development Setup

### Prerequisites

- Rust 1.87+ (`rustup default stable`)
- For mobile bindings: Android NDK, Xcode
- For regenerating UniFFI bindings: `cargo install uniffi --version 0.30.0 --features cli --locked` (the CLI version must match the workspace `uniffi = "0.30"` pin; CI uses this exact command)

### Clone and Build

```bash
git clone https://github.com/Offline-Protocol/offline-protocol-sdk
cd offline-protocol-sdk
cargo build --workspace
cargo test --workspace
```

## Code Quality Standards

### Before Every Commit

Run these checks (they must all pass):

```bash
# 1. Build without errors
cargo build --workspace

# 2. All tests pass
cargo test --workspace

# 3. No clippy warnings
cargo clippy --workspace -- -D warnings

# 4. Code is formatted
cargo fmt --all
```

### Safety Requirements

- **Core crates**: `#![deny(unsafe_code)]` - zero unsafe code allowed
- **FFI crate**: Unsafe code is permitted but must:
  - Have SAFETY comments explaining why it's safe
  - Validate all pointers (null checks)
  - Catch all panics (`catch_unwind`)
  - Be reviewed by maintainers

### Testing Requirements

- New features must include tests
- Bug fixes must include regression tests
- Aim for >80% code coverage
- Integration tests for end-to-end scenarios

## Commit Message Format

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

[optional body]
```

**Types**:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation
- `test`: Adding tests
- `refactor`: Code refactoring
- `perf`: Performance improvement
- `chore`: Build/tooling changes

**Scopes**:
- `core`: offline-protocol-core
- `sealed`: offline-protocol-sealed (envelope codec, address derivation, signing payloads)
- `leaf`: offline-protocol-leaf (a constrained device as a never-committing MLS member)
- `transport`: offline-protocol-transport
- `router`: offline-protocol-router (DORS)
- `reliability`: offline-protocol-reliability
- `mls`: offline-protocol-mls (MLS encryption)
- `services`: offline-protocol-services (service discovery)
- `protocol`: offline-protocol (main API)
- `uniffi`: offline-protocol-uniffi (UniFFI bindings)
- `bindings`: Platform bindings

**Examples**:
```
feat(router): add congestion-aware path selection
fix(reliability): correct retry backoff calculation
docs(api): update configuration reference
test(dors): add tests for transport switching
```

## Code Organization

### Adding a New Feature

1. **Core Logic**: Implement in appropriate Rust crate (100% safe)
2. **Tests**: Add comprehensive unit tests
3. **UniFFI**: Expose via UDL if needed for mobile platforms
4. **Regenerate**: After any UDL change run `./scripts/generate-bindings.sh` and commit
   **all three** generated files (Swift, Kotlin, Python). They are one artifact set off one
   UDL and carry the FFI checksums of the library they were generated against — committing a
   subset leaves the rest describing a different ABI, which no build catches; the app fails
   at its first FFI call instead.
5. **Bindings**: Update platform bindings (React Native, etc.)
6. **Docs**: Update README and relevant docs
7. **Commit**: Use conventional commits format

### File Structure

```
offline-protocol-sdk/
├── crates/          # Rust crates (core logic)
│   ├── offline-protocol-core/
│   ├── offline-protocol-sealed/
│   ├── offline-protocol-leaf/
│   ├── offline-protocol-transport/
│   ├── offline-protocol-router/
│   ├── offline-protocol-reliability/
│   ├── offline-protocol-mls/
│   ├── offline-protocol-services/
│   ├── offline-protocol-data/
│   ├── offline-protocol/
│   ├── offline-protocol-uniffi/
│   └── offline-protocol-bench/
├── bindings/        # Platform bindings (React Native)
├── docs/            # Documentation
└── examples/        # Example applications
```

## Pull Request Process

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Make your changes
4. Run quality checks (build, test, clippy, fmt)
5. Commit with conventional commits
6. Push to your fork
7. Create a Pull Request

### PR Checklist

- [ ] Tests pass (`cargo test --workspace`)
- [ ] No clippy warnings (`cargo clippy --workspace -- -D warnings`)
- [ ] Code formatted (`cargo fmt --all`)
- [ ] Documentation updated
- [ ] Conventional commit messages
- [ ] No breaking changes (or clearly documented)

## Development Workflow

### Running Tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test --package offline-protocol-core

# Specific test
cargo test test_message_creation

# With output
cargo test -- --nocapture
```

### Linting

```bash
# Check for issues
cargo clippy --workspace

# Fix automatically fixable issues
cargo clippy --workspace --fix

# Strict mode (required for CI)
cargo clippy --workspace -- -D warnings
```

### Formatting

```bash
# Format all code
cargo fmt --all

# Check formatting without applying
cargo fmt --all -- --check
```

### Documentation

```bash
# Generate and open docs
cargo doc --workspace --open

# Check doc comments — this is what the Rustdoc CI job runs. Keep the flag:
# without it a broken intra-doc link only prints a warning and still exits 0,
# so the check passes locally and fails in CI.
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

### Running Benchmarks

Run the complete Criterion suite and build the same performance dashboard shown
by CI:

```bash
cargo bench --package offline-protocol-bench --bench '*' --locked
python3 scripts/benchmark_report.py \
  --criterion-dir target/criterion \
  --metadata benches/benchmark-metadata.json \
  --output target/criterion/benchmark-summary.md \
  --json-output target/criterion/benchmark-results.json
```

Open `target/criterion/report/index.html` for Criterion's interactive charts or
read `target/criterion/benchmark-summary.md` for the compact overview. The
dashboard reports median latency with 95% confidence intervals, throughput,
variance, outliers, budget status, and—when Criterion has a baseline—relative
change. Latency bars are relative to the slowest result and meaningful only
within their benchmark group; there is intentionally no aggregate score across
unrelated operations.

Performance budgets and benchmark descriptions live in
`benches/benchmark-metadata.json`. CI evaluates budgets against the upper bound
of the confidence interval. Hosted-runner results are useful for spotting a
directional change, but important regressions should be reproduced on stable,
controlled hardware.

For a controlled local before/after comparison, save a named baseline before
editing and compare against it afterward:

```bash
cargo bench --package offline-protocol-bench --bench '*' --locked -- \
  --save-baseline before
# Make the change, then run:
cargo bench --package offline-protocol-bench --bench '*' --locked -- \
  --baseline-lenient before
```

## Cutting a Release

One number identifies a release across every channel: the `vX.Y.Z` git tag, the
`@offline-protocol/mesh-sdk` npm package, the `offline-protocol*` crates on
crates.io, and the GitHub release assets. Pushing the tag is what publishes, so
everything below happens on a `chore/release-X.Y.Z` branch that merges first.

Version files to bump together:

| File | How |
| --- | --- |
| `Cargo.toml` | `[workspace.package].version` **and** the ten internal-dependency versions in `[workspace.dependencies]`. A partial *minor* bump fails to resolve immediately and locally; a partial *patch* bump resolves silently, because `version = "X.Y.Z"` is a caret requirement that a sibling one patch ahead still satisfies. Harmless if it ships, but move them together |
| `crates/offline-protocol-sealed/Cargo.toml`, `crates/offline-protocol-leaf/Cargo.toml` | Four more internal-dependency versions live here, not in the workspace table: the dual `std`/`no_std` crates declare their dependencies locally, because cargo silently ignores `default-features = false` on an inherited dependency (ADR [0020](docs/adr/0020-core-compiles-without-std.md), [0022](docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md)). Miss these and the workspace stops resolving; `local_dep_versions_match_the_workspace_table` is the guard |
| `Cargo.lock` | any `cargo` command refreshes it once the manifests change |
| `tools/embedded-footprint/Cargo.lock`, `tools/mls-interop/Cargo.lock` | Separate workspaces that depend on the SDK crates by path, so their lockfiles pin the versions too. CI builds both with `--locked`, so a stale one fails the run. `cargo metadata --manifest-path <tool>/Cargo.toml` refreshes each |
| `bindings/python/pyproject.toml` | `version` — release.yml gates this one against the tag too |
| `bindings/react-native/package.json` + `package-lock.json` | `npm version X.Y.Z --no-git-tag-version --allow-same-version` from `bindings/react-native/` |
| `THIRD-PARTY-NOTICES.md` (×3) | `scripts/generate-third-party-notices.sh` — the notices list our own crates by version, and the CI drift gate fails on a stale copy |

Then the prose: `CHANGELOG.md` (replace `## [Unreleased]` with
`## [X.Y.Z] — <date>`, leaving no empty `[Unreleased]` behind), `SECURITY.md`
(the supported-versions table — minor bumps only; a patch stays on its line),
and `docs/UPGRADING.md` (the current-line reference, and a labelled subsection
for anything that compiles fine but behaves differently).

Then archive: the working `CHANGELOG.md` holds unreleased changes plus **one**
release, so cutting X.Y.Z moves the now-previous release's section into
`docs/changelog/<major>.<minor>.md` (creating the file with its title and
release table if the series is new) and adds a row to both archive tables, the
one in [`docs/changelog/README.md`](docs/changelog/README.md) and the one at the
foot of `CHANGELOG.md`. Relative links move with the text and break silently:
`./docs/UPGRADING.md` becomes `../UPGRADING.md` and `./CONTRIBUTING.md` becomes
`../../CONTRIBUTING.md` once the section lives two directories down.

To rehearse the whole thing, push a `vX.Y.Z-rc.N` tag. Every gate runs, every
crate is packaged and verify-built, and npm publishes under the `next`
dist-tag — but crates.io gets nothing, because those versions are immutable and
an rc must never burn the number the final tag needs. Point the rc at the same
`X.Y.Z` the workspace already carries; the gate compares release cores and
ignores the suffix.

A `workflow_dispatch` run with `dry_run: true` rehearses something an rc tag
cannot, and it works from a branch: it performs the real npm OIDC exchange and
fails if the registry does not recognise this workflow's identity, without
uploading anything. Pass an explicit `version` when you do, because a dry run
without one resolves to `0.0.0-dev` and `scripts/prepare-npm.sh` rejects that
before the rehearsal is reached.

Four failure modes worth naming:

- **`release.yml` refuses to publish a tag whose number does not match
  `[workspace.package].version`** (and `pyproject.toml`'s). That gate exists
  because crates.io versions are immutable — a wrong number can be yanked but
  never corrected, so the release has to fail before it uploads rather than
  after. It runs as its own `version-gate` job ahead of *both* publish jobs, so
  a forgotten bump stops npm as well: caught inside the crates job alone, it
  would fail there while npm published the number anyway, and that split is one
  of the permanent ones below. Because it lands before anything ships, deleting
  and re-pushing the tag is the correct fix here — the one point in the release
  where re-tagging is still safe.
- **The npm version is written from the tag at release time; the Cargo version
  is not.** Rewriting manifests in CI would invalidate `Cargo.lock` and leave
  the published `.crate` no longer matching the tag it names, so the repo is
  the source of truth and the tag is checked against it.
- **Never force-move a released tag — that recovery move stopped working when
  crates.io publishing landed.** It used to be the cheap fix for a failure
  after the GitHub release (the `v0.20.1` npm-provenance 422, for instance):
  move the tag, re-run, done. Now it silently splits the release across
  registries in either direction. If the tag already published crates, the
  re-run skips them as already-present and crates.io keeps serving the *old*
  source while npm and the release assets get the new. If the tag predates
  crates.io publishing — every tag through `v0.20.1` — moving it onto a commit
  that carries the `publish-crates` job passes every gate and publishes crates
  built from source that the npm package of the same number never contained.
  Both are permanent. Recover by cutting the next patch version instead.
- **npm publishing depends on configuration that lives at the registry, not in
  this repository.** The package authenticates with trusted publishing, which
  npm matches against this repository plus the workflow *filename*
  `release.yml`. Renaming that file, or factoring the publish step out into a
  reusable `workflow_call` workflow (npm validates the *calling* workflow's
  name), revokes publishing with nothing in the diff to review, and the failure
  surfaces as a 404 at the last step of a release. It is worth naming because
  the recovery above no longer exists: a tag that has already published crates
  cannot be moved, so a publish that fails here costs a patch version. The
  `dry_run` dispatch is what makes that cheap to check beforehand.

## Architecture Decisions

When making significant changes:

1. **Safety First**: Prefer safe Rust over unsafe
2. **Performance**: Measure before optimizing
3. **Simplicity**: Clear code over clever code
4. **Testing**: Test coverage for all paths
5. **Documentation**: Public APIs must be documented

## Questions?

- Open a [Discussion](https://github.com/Offline-Protocol/offline-protocol-sdk/discussions)
- Ask in [Issues](https://github.com/Offline-Protocol/offline-protocol-sdk/issues)

## License & Contributor Agreement

This project is **dual-licensed** under the GNU Affero General Public License
v3.0 (see [LICENSE](LICENSE)) **or** a separate Commercial License (see
[LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md)).

For the dual-licensing model to work, we need each contributor to grant the
maintainers the right to sublicense contributed code under the Commercial
License alongside the AGPL. That grant is collected via a Contributor License
Agreement — see [CLA.md](CLA.md) for the full terms and the rationale.

**On your first PR**, our CLA bot will post a link back to `CLA.md` and ask you
to comment, exactly:

> I have read the CLA Document and I hereby sign the CLA

You only sign once; subsequent contributions are auto-recognized. PRs cannot be
merged until the CLA check is green.

If you are contributing on behalf of an employer, confirm with them that you
are authorized to grant these rights before signing.
