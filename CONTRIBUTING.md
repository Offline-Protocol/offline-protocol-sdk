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
│   ├── offline-protocol-telemetry-wire/
│   ├── offline-protocol/
│   ├── offline-protocol-uniffi/
│   └── offline-protocol-bench/
├── bindings/        # Platform bindings (React Native, Python, Swift, Kotlin)
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
crates.io, the GitHub release assets (the Python wheels among them), the Swift
package's `X.Y.Z` tag in `Offline-Protocol/offline-protocol-swift`,
`com.offlineprotocol:offline-protocol-sdk` on Maven Central,
`offline-protocol-sdk` on PyPI, and the service image
`ghcr.io/offline-protocol/offline-protocol-service`, which installs that PyPI
release. Pushing the tag is what publishes, so everything below happens on a
`chore/release-X.Y.Z` branch that merges first. Maven Central and PyPI are
switched on one by one, the image follows PyPI's switch, and the Swift package
is pulled rather than pushed; see
[the native packages and PyPI](#the-native-packages-and-pypi).

Version files to bump together:

| File | How |
| --- | --- |
| `Cargo.toml` | `[workspace.package].version` **and** the eleven internal-dependency versions in `[workspace.dependencies]`. A partial *minor* bump fails to resolve immediately and locally; a partial *patch* bump resolves silently, because `version = "X.Y.Z"` is a caret requirement that a sibling one patch ahead still satisfies. Harmless if it ships, but move them together |
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

"Silently" is literal, and nothing in CI catches it: the 0.22.0 archive shipped
eight `](docs/…)` links that had resolved from the repository root and resolve
from nowhere once the text sits in `docs/changelog/`, and they stayed broken
across three releases. So check the whole tree rather than the section you
moved, by resolving every `](path#anchor)` in every tracked markdown file
against the headings that actually exist. Prove the move itself lossless the
other way round: re-extract the section from `git show HEAD:CHANGELOG.md`, apply
the same rewrite, and diff it against what landed in the series file, which
should differ on exactly the link lines and nothing else.

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

### The native packages and PyPI

The Swift package, the Android library and the wheels are built from the
release's own libraries and tested before anything publishes: the Swift package
on a simulator with an application built against it, the Android library with
all four ABIs and a minified application, and each wheel installed by pip and
run through the whole suite on the system it is tagged for. All of that runs on
every release and every dry run, and a failure stops npm and crates.io too.

Maven Central and PyPI then publish after the `release` job, only from a
`v*` tag that is not a dry run, and only when the channel's repository
variable is `true`. A channel switched on with its secret missing fails the
run rather than skipping, because a skipped channel is a release that looks
shipped. The service image publishes after PyPI, under the same switch,
because it installs the wheels PyPI just served: `service-image.yml` waits
until pip can download both Linux wheels of the release, builds
`bindings/python/docker` for amd64 and arm64 with `SDK_VERSION` set to that
number, and pushes it. A prerelease is pushed under its own tag
(`0.30.0-rc.1`) only; `X.Y` and `latest` move only to the newest final
release.

| Channel | Switch (variable) | Credentials (secrets) | A dry run with it on |
| --- | --- | --- | --- |
| Maven Central | `MAVEN_CENTRAL_PUBLISH` | `MAVEN_CENTRAL_USERNAME` and `MAVEN_CENTRAL_PASSWORD` (a Central Portal user token), `MAVEN_SIGNING_KEY` (armored private key) and `MAVEN_SIGNING_KEY_PASSWORD` | uploads a deployment Central validates, then drops it |
| PyPI | `PYPI_PUBLISH` | none: trusted publishing from `release.yml` in the `pypi` environment | checks the wheels' metadata only: PyPI has no rehearsal |
| Service image (ghcr.io) | `PYPI_PUBLISH`, since it installs what PyPI published | none: the workflow's own `GITHUB_TOKEN` with `packages: write` | not built: it needs the release on PyPI |

**The Swift package is pulled, not pushed.** Nothing in this repository can
write to `Offline-Protocol/offline-protocol-swift`, and no credential for it
exists anywhere. The release attaches the assembled package
(`offline-protocol-X.Y.Z-swift-package.tar.gz`) and the XCFramework archive
its manifest names, both attested. The distribution repository's own
workflow downloads the two, verifies the attestations against this
repository's `release.yml`, and commits and tags the package with the token
every workflow holds for its own repository. After a release is green, run
it:

```bash
gh workflow run publish.yml -R Offline-Protocol/offline-protocol-swift -f version=X.Y.Z
```

Its schedule also publishes the newest release every six hours if nobody
did, and skips a release that has no package (every release before 0.28.0).
The logic is `scripts/publish-swift-package.sh` here, checked out from the
release's tag by that workflow, so it is tested in this repository and copied
nowhere. A dry run of this workflow assembles the package and parses its
manifest with Swift Package Manager; `dry_run` on the distribution
repository's workflow rehearses the pull without pushing.

One-time setup, outside this repository (none for the Swift package):

- **Maven Central**: a Central Portal account, the `com.offlineprotocol`
  namespace verified (a DNS TXT record on `offlineprotocol.com`), a user token,
  and the signing key's public half on the keyservers. Central checks every
  signature against the keyservers, so a key that is not there fails the
  rehearsal, which is where it should fail. The steps are in
  [Maven Central, once](#maven-central-once).
- **PyPI**: a trusted publisher for `offline-protocol-sdk` naming this
  repository, the workflow `release.yml` and the environment `pypi`. As with
  npm, renaming the file revokes it.
- **The service image**: nothing before the first run. A package a workflow
  creates in the organization can start out private, so after the first
  push an organization admin opens the `offline-protocol-service` package's
  settings, makes it public, and checks it is linked to this repository
  (the image's `org.opencontainers.image.source` label links it).

Four things here cannot be undone, and each script refuses rather than
guesses:

- **A Swift package tag never moves.** Swift Package Manager records the
  revision behind every version it resolves, so a moved tag breaks every
  consumer that resolved it. `publish-swift-package.sh` accepts a version that
  is already tagged only when the tag holds exactly what it would publish (a
  re-run) and refuses one that differs. A bad Swift release is answered by the
  next version. Never edit the distribution repository by hand: the next
  release replaces its whole tree.
- **A version on Maven Central or PyPI is permanent**, including a release
  candidate. An rc publishes as `X.Y.Z-rc.N` on Central and `X.Y.ZrcN` on
  PyPI (the wheel is numbered from the tag, in Python's spelling), so it never
  takes the number the final release needs. A re-run skips a version that is
  already on PyPI, and one already on Maven Central once `repo1.maven.org`
  serves it, which is minutes after the Portal publishes; a re-run inside
  that window uploads a duplicate the Portal refuses, which turns the job
  red and changes nothing.
- **An image version is not replaced, though the registry would allow it.**
  GHCR accepts a second push of `X.Y.Z`, and a deployment that pinned it
  would then run something else under the same number. So
  `service-image.yml` refuses before building when `X.Y.Z` exists, refuses
  a run by hand from any branch but the default one (the image is built
  from that ref's Dockerfile), and treats any registry answer but "not
  found" as a reason to stop. Its `overwrite` input lifts the first check,
  and is for one case only: an image whose checks failed after the push.
  That is also the recovery when `publish-image` fails in a release: run
  Actions > Service Image from `main` with the release tag, and
  `overwrite` only if the push itself happened. A re-run of the whole
  release after `publish-image` went green turns that job red, and changes
  nothing.
- **The wheels' platform tags are read from the libraries.** A Linux wheel is
  tagged `manylinux_2_N` for the newest glibc its library needs, which is the
  glibc of the image that built it. That counts the `GLIBC_ABI_*` markers as
  well as the symbol versions: a library linked with packed relative
  relocations needs `GLIBC_ABI_DT_RELR`, which is glibc 2.36 whatever its
  symbols say, and `build-wheel.sh` refuses a marker it does not know rather
  than ignore it. Building on a newer image, or a linker that starts packing
  relocations by default, raises the floor for every Linux user; that is a
  change to say in the changelog. The macOS tag is a chosen floor, 14.0 (the
  oldest arm64 runner the suite runs on), and the library's own minimum must
  not exceed it. Every wheel's architecture is read from its library's
  header, so a library from the wrong build is refused rather than wheeled
  under a correct name.

#### Maven Central, once

The `publish-maven` job in `release.yml` is complete and switched off: every
release builds, tests and writes out the library, and stops there while
`MAVEN_CENTRAL_PUBLISH` is not `true`. What it uploads is the AAR (classes,
the UniFFI bindings, `jni/<abi>/libuniffi_offline_protocol.so` for
arm64-v8a, armeabi-v7a, x86 and x86_64, and the license files), its POM,
the Gradle module file, the sources jar and a javadoc jar, each signed with
gpg and given MD5 and SHA-1 checksums by `scripts/maven-central-bundle.sh`,
as one bundle through the Central Portal Publisher API
(`scripts/maven-central-upload.sh`, `https://central.sonatype.com/api/v1/publisher`).
The OSSRH endpoints are not used anywhere.

Switching it on takes an organization owner, once. Decide the tier first,
before the DNS record or the key. From 2026-10-01 Sonatype requires Maven
Central Publisher Pro for an artifact of "commercial nature", one that
"supports the adoption, integration, or operation of your organization's
commercial product or service", whatever its publishing volume, and it names
service-dependent SDKs among its examples. The license does not settle it:
Sonatype says no single signal, license included, decides the question, and
this library's POM itself points at a commercial license. There is nothing to
set in the Portal account either way. The choice is one of two requests:

- **Publisher Pro**: through Sonatype's
  [Maven Central Pro contact page](https://www.sonatype.com/products/maven-central-pro/contact).
  No price is published.
- **Excluded as community**: mail central-support@sonatype.com asking that
  `com.offlineprotocol` be excluded from commercial classification, naming the
  organization, the group id, why it is not part of a commercial
  go-to-market effort, and which of the organization's artifacts remain
  commercial.

The rules are on Sonatype's
[publishing limits page](https://central.sonatype.org/publish/maven-central-publishing-limits/).
Then, in this order:

1. **The account and the namespace.** Sign in at
   <https://central.sonatype.com> with an account the company controls (not a
   personal one: it owns the namespace). Under Namespaces, add
   `com.offlineprotocol`. The Portal shows a verification key: add it as a DNS
   TXT record on `offlineprotocol.com` and press Verify. Keep the record until
   the namespace shows as verified. Then open Publishing, Usage Center: it is
   where Sonatype states the monthly file, size and release limits, and a
   release (the AAR alone is about 21 MiB) counts against all three.
2. **A Portal user token.** Under the account menu, View Account, Generate User
   Token. It is a name and a secret, shown once. They are
   `MAVEN_CENTRAL_USERNAME` and `MAVEN_CENTRAL_PASSWORD` below; the account's
   own login password is never used.
3. **A release signing key**, used for nothing else, on a trusted machine:

   ```bash
   gpg --quick-gen-key "Offline Protocol releases <ADDRESS@offlineprotocol.com>" rsa4096 sign 2y
   FPR=<the fingerprint gpg printed>
   gpg --keyserver hkps://keyserver.ubuntu.com --send-keys "$FPR"
   gpg --keyserver hkps://keys.openpgp.org --send-keys "$FPR"
   gpg --armor --export-secret-keys "$FPR" > release-signing-key.asc
   ```

   Use an address the team reads. keys.openpgp.org publishes the key without
   its user id until that address confirms the mail it sends; Central needs
   only the key, and keyserver.ubuntu.com keeps the user id either way. Keep
   the revocation certificate gpg wrote under `openpgp-revocs.d` and a copy of
   the private key offline, and extend the expiry (then send the key again)
   before it lapses: an expired key fails the rehearsal.
4. **The secrets**, as repository secrets on this repository (Settings,
   Secrets and variables, Actions). These are the names `release.yml` reads:

   | Secret | Value |
   | --- | --- |
   | `MAVEN_CENTRAL_USERNAME` | The user token's name, from step 2 |
   | `MAVEN_CENTRAL_PASSWORD` | The user token's secret, from step 2 |
   | `MAVEN_SIGNING_KEY` | The whole of `release-signing-key.asc`, armored, exactly one secret key |
   | `MAVEN_SIGNING_KEY_PASSWORD` | The key's passphrase; leave it unset for a key without one |

   ```bash
   gh secret set MAVEN_CENTRAL_USERNAME -R Offline-Protocol/offline-protocol-sdk
   gh secret set MAVEN_CENTRAL_PASSWORD -R Offline-Protocol/offline-protocol-sdk
   gh secret set MAVEN_SIGNING_KEY -R Offline-Protocol/offline-protocol-sdk < release-signing-key.asc
   # Only for a key with a passphrase:
   gh secret set MAVEN_SIGNING_KEY_PASSWORD -R Offline-Protocol/offline-protocol-sdk
   ```

   Then delete `release-signing-key.asc` from the machine.
5. **The switch**, as a repository variable:

   ```bash
   gh variable set MAVEN_CENTRAL_PUBLISH --body true -R Offline-Protocol/offline-protocol-sdk
   ```

6. **A rehearsal before the next tag.** Run Release & Publish from `main` with
   `dry_run` checked and the version box empty. The Publish to Maven Central
   job uploads the library as a deployment Central validates and then drops,
   so a wrong token, an unverified namespace or a key the keyservers do not
   have fails there, with the Portal's reason in the log, and nothing is
   released. The form calls the version required, but left empty it becomes
   `0.0.0-dev`, which never collides with a release. Never name a version
   that is already on Central: the Portal refuses the duplicate and the
   rehearsal fails for that reason alone.

The first release tag after that publishes to Maven Central, permanently. The
library a release built while the channel was off is kept for a day (the
run's `android-library-maven` artifact); within that day, re-running only that
run's Publish to Maven Central job, once the switch is on, publishes it.

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
