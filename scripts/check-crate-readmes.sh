#!/usr/bin/env bash
# Asserts every publishable crate carries a README.
#
# crates.io renders the README from the .crate archive, and a published version
# is immutable -- there is no edit-metadata API, so a crate that ships without
# one shows nothing but its one-line `description` on its page until the *next*
# release goes out. That is precisely how all eight crates reached the registry
# bare: cargo infers `readme = "README.md"` when the file sits beside
# Cargo.toml and silently infers nothing when it does not, so the omission
# produces no warning at package time, no failure at publish time, and is only
# visible by looking at the rendered page afterwards.
#
# A crate added later inherits nothing from the others, which makes this the
# same shape as the license-copy assertion next door: cheap to check, invisible
# until too late to fix in place.
set -euo pipefail
cd "$(dirname "$0")/.."

status=0

for manifest in crates/*/Cargo.toml; do
  crate_dir="$(dirname "$manifest")"
  # Harness and internal crates opt out via publish = false, as cargo enforces.
  # Nothing is rendered for something never distributed.
  if grep -qE '^publish[[:space:]]*=[[:space:]]*false' "$manifest"; then
    continue
  fi

  # An explicit `readme` key overrides the inferred README.md, so follow it
  # rather than checking a file cargo may not be reading. `readme = false`
  # suppresses the README outright -- legal, but not what a published crate
  # wants, so name it as the error it is here.
  declared="$(sed -nE 's/^[[:space:]]*readme[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$manifest" | head -n1)"
  if grep -qE '^[[:space:]]*readme[[:space:]]*=[[:space:]]*false' "$manifest"; then
    echo "error: $manifest sets readme = false but is publishable" >&2
    echo "       crates.io would render no README for it -- drop the key" >&2
    status=1
    continue
  fi

  readme_path="$crate_dir/${declared:-README.md}"
  if [ ! -f "$readme_path" ]; then
    echo "error: $crate_dir is publishable but has no README at $readme_path" >&2
    echo "       add one -- crates.io renders it from the archive, and a" >&2
    echo "       published version cannot be amended afterwards" >&2
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "Every publishable crate ships a README."
fi
exit "$status"
