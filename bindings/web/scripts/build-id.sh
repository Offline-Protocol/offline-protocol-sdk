#!/usr/bin/env bash
# Writes bindings/web/pkg/build-id.js: one id over every file the lobby's
# service worker caches, so a change to any of them installs a new cache
# whole (docs/bridges/web.md, W15). The service worker's own script is the
# same in every build, so this id is all a browser's update check compares:
# a served file that changed under an old id stays cached on every device
# that opened the page, online or not. build.sh runs this last; run it
# after any change to a served file that needs no Rust build (the SDK's
# JavaScript, the lobby), and before serving it.
#
# The whole lobby directory rather than a list, so a file added beside the
# page cannot be left out of the id; the README in it costs a harmless extra
# update.
# ponytail: hashes the lobby's directory; take it as an argument when a
# second page caches the SDK.
set -euo pipefail
cd "$(dirname "$0")/../../.."

id="$(cat bindings/web/pkg/offline_protocol_web.js bindings/web/pkg/offline_protocol_web_bg.wasm \
  bindings/web/src/*.js examples/web-lobby/* | shasum -a 256 | cut -c1-16)"
echo "self.BUILD_ID = \"$id\";" > bindings/web/pkg/build-id.js
echo "bindings/web/pkg/build-id.js: $id"
