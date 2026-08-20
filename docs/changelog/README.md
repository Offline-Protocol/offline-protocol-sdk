# Changelog archive

Release notes for versions before the current release, one file per minor
series. The working changelog, covering unreleased changes and the current
release, is [CHANGELOG.md](../../CHANGELOG.md).

| Series | Releases | First | Last |
|--------|----------|-------|------|
| [0.22.x](0.22.md) | 0.22.0 | 2026-08-18 | 2026-08-18 |
| [0.21.x](0.21.md) | 0.21.0 | 2026-08-13 | 2026-08-13 |
| [0.20.x](0.20.md) | 0.20.1, 0.20.0 | 2026-08-07 | 2026-08-07 |
| [0.19.x](0.19.md) | 0.19.0 | 2026-08-04 | 2026-08-04 |
| [0.18.x](0.18.md) | 0.18.3, 0.18.2, 0.18.1, 0.18.0 | 2026-07-31 | 2026-08-03 |
| [0.17.x](0.17.md) | 0.17.0 | 2026-07-30 | 2026-07-30 |
| [0.16.x](0.16.md) | 0.16.6 through 0.16.0 | 2026-07-24 | 2026-07-28 |
| [0.15.x](0.15.md) | 0.15.0 | 2026-07-20 | 2026-07-20 |
| [0.14.x](0.14.md) | 0.14.0 | 2026-07-16 | 2026-07-16 |
| [0.13.x](0.13.md) | 0.13.1, 0.13.0 | 2026-07-13 | 2026-07-14 |
| [0.12.x](0.12.md) | 0.12.0 | 2026-07-13 | 2026-07-13 |
| [0.11.x](0.11.md) | 0.11.1, 0.11.0 | 2026-07-01 | 2026-07-12 |
| [0.10.x](0.10.md) | 0.10.0 | 2026-04-13 | 2026-04-13 |
| [0.9.x](0.9.md) | 0.9.4 through 0.9.0 | 2026-03-20 | 2026-03-27 |
| [0.8.x](0.8.md) | 0.8.0 | 2026-03-19 | 2026-03-19 |

Releases before v0.7.1 are not covered by this changelog.

## Archiving procedure

When cutting a release, the previous release moves out of the working file:

1. Cut the release as usual in `CHANGELOG.md`.
2. Move the now-previous release's section into
   `docs/changelog/<major>.<minor>.md`, creating the file with its title and
   release table if the series is new.
3. Update the archive table above and the one at the foot of `CHANGELOG.md`.

The working file should hold unreleased changes plus one release. Anything more
and it grows without bound again.
