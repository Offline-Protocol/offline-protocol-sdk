<!--
Thanks for contributing to the Offline Protocol SDK!
Please fill out the sections below. Delete anything that doesn't apply.
-->

## Summary

<!-- What does this PR do, and why? -->

## Related issues

<!-- e.g. "Closes #123" / "Refs #456". Use "Closes" to auto-close on merge. -->

Closes #

## Type of change

<!-- Mark with an [x] all that apply. Matches our Conventional Commit types. -->

- [ ] `feat` — new feature
- [ ] `fix` — bug fix
- [ ] `docs` — documentation only
- [ ] `refactor` — code change that neither fixes a bug nor adds a feature
- [ ] `perf` — performance improvement
- [ ] `test` — adding or correcting tests
- [ ] `chore` — build, tooling, or dependency changes

## Checklist

<!-- These mirror the CI gates — PRs that fail them will be blocked. -->

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo-deny` is satisfied (no new license/advisory violations)
- [ ] Commits follow [Conventional Commits](https://www.conventionalcommits.org/) (`<type>(<scope>): <subject>`)
- [ ] Docs / `CHANGELOG.md` updated where relevant
- [ ] No new `unsafe` in core crates (only the `offline-protocol-uniffi` FFI boundary may use it, with SAFETY comments)
- [ ] If the UniFFI UDL changed, bindings were regenerated (`./scripts/generate-bindings.sh`) and **all three** — Swift, Kotlin, Python — committed together; a partial set fails at runtime, not build time

## Breaking changes

<!-- Describe any breaking API/wire/config changes and the migration path, or "None". -->

None

## Notes for reviewers

<!-- Anything that helps review: design decisions, areas needing extra scrutiny, follow-ups. -->

---

<!--
First-time contributor? Our CLA bot will comment on this PR with a link to
CLA.md. To sign, comment exactly:

  I have read the CLA Document and I hereby sign the CLA

You only sign once. PRs cannot be merged until the CLA check is green.
-->
