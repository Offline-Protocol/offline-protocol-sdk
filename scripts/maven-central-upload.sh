#!/usr/bin/env bash

# Upload a bundle to the Maven Central Portal and wait for its verdict.
#
#   publish    upload with publishingType=AUTOMATIC, which releases the
#              deployment as soon as it validates, and wait until it is
#              publishing. A version on Maven Central is permanent: it can
#              never be replaced or deleted.
#   rehearse   upload with publishingType=USER_MANAGED, wait until it
#              validates, then drop it. That exercises the token, the
#              namespace, the signature against the published key and every
#              POM requirement, and releases nothing. A dry run of the
#              release does this, so a misconfigured channel fails before a
#              tag is cut rather than after npm has published.
#
# A re-run of a release whose version is already on Maven Central does not
# upload again: the Portal would refuse the duplicate and turn a shipped
# release red.
#
# Usage:
#   MAVEN_CENTRAL_USERNAME=<token name> MAVEN_CENTRAL_PASSWORD=<token> \
#     bash scripts/maven-central-upload.sh <bundle.zip> <publish|rehearse> <version>
#
# Environment (for tests):
#   CENTRAL_API         defaults to https://central.sonatype.com/api/v1/publisher
#   CENTRAL_REPOSITORY  defaults to https://repo1.maven.org/maven2
#   POLL_SECONDS        defaults to 15
#   POLLS               defaults to 120 (thirty minutes)

set -euo pipefail

die() {
  echo "ERROR: $*" >&2
  exit 1
}

[ "$#" -eq 3 ] || die "usage: $0 <bundle.zip> <publish|rehearse> <version>"
BUNDLE="$1"
MODE="$2"
VERSION="$3"
: "${MAVEN_CENTRAL_USERNAME:?MAVEN_CENTRAL_USERNAME must hold the Portal user token name}"
: "${MAVEN_CENTRAL_PASSWORD:?MAVEN_CENTRAL_PASSWORD must hold the Portal user token}"
[ -f "$BUNDLE" ] || die "no such bundle: $BUNDLE"

API="${CENTRAL_API:-https://central.sonatype.com/api/v1/publisher}"
REPOSITORY="${CENTRAL_REPOSITORY:-https://repo1.maven.org/maven2}"
POLL_SECONDS="${POLL_SECONDS:-15}"
POLLS="${POLLS:-120}"
POM="$REPOSITORY/com/offlineprotocol/offline-protocol-android/$VERSION/offline-protocol-android-$VERSION.pom"

case "$MODE" in
  publish) PUBLISHING_TYPE=AUTOMATIC ;;
  rehearse) PUBLISHING_TYPE=USER_MANAGED ;;
  *) die "the mode must be publish or rehearse, not '$MODE'" ;;
esac

if [ "$MODE" = publish ] && curl -sSfI "$POM" >/dev/null 2>&1; then
  echo "::notice title=Already on Maven Central::offline-protocol-android $VERSION is already on Maven Central, and a version there is permanent. Nothing uploaded."
  exit 0
fi

TOKEN="$(printf '%s:%s' "$MAVEN_CENTRAL_USERNAME" "$MAVEN_CENTRAL_PASSWORD" | base64 | tr -d '\n')"
# Derived from two secrets, so the runner does not know to hide it.
[ -z "${GITHUB_ACTIONS:-}" ] || echo "::add-mask::$TOKEN"
AUTH="Authorization: Bearer $TOKEN"

RESPONSE="$(mktemp)"
trap 'rm -f "$RESPONSE"' EXIT

# --fail-with-body keeps the Portal's explanation on a refusal.
if ! curl -sS --fail-with-body -X POST -H "$AUTH" \
  -F "bundle=@$BUNDLE;type=application/octet-stream" \
  "$API/upload?publishingType=$PUBLISHING_TYPE&name=offline-protocol-android-$VERSION" >"$RESPONSE"; then
  echo "The Portal refused the upload:" >&2
  cat "$RESPONSE" >&2
  echo >&2
  die "upload failed"
fi
DEPLOYMENT="$(tr -d '[:space:]' <"$RESPONSE")"
[[ "$DEPLOYMENT" =~ ^[0-9A-Za-z-]+$ ]] || die "the Portal answered the upload with something that is not a deployment id: $DEPLOYMENT"
echo "Deployment $DEPLOYMENT ($PUBLISHING_TYPE)"

drop() {
  curl -sS --fail-with-body -X DELETE -H "$AUTH" "$API/deployment/$DEPLOYMENT" >/dev/null ||
    echo "::warning title=Deployment not dropped::Deployment $DEPLOYMENT could not be dropped. Drop it at https://central.sonatype.com/publishing/deployments before it is published by hand."
}

STATE=""
for poll in $(seq 1 "$POLLS"); do
  curl -sS --fail-with-body -X POST -H "$AUTH" "$API/status?id=$DEPLOYMENT" >"$RESPONSE" ||
    die "the status request failed: $(cat "$RESPONSE")"
  STATE="$(jq -r '.deploymentState // empty' "$RESPONSE")"
  echo "Deployment $DEPLOYMENT is $STATE (poll $poll of $POLLS)"
  case "$STATE" in
    FAILED)
      echo "The Portal refused the deployment:" >&2
      jq -r '.errors // {} | to_entries[] | "  \(.key): \(.value | if type == "array" then join("; ") else tostring end)"' "$RESPONSE" >&2 || cat "$RESPONSE" >&2
      [ "$MODE" = rehearse ] && drop
      die "the deployment failed validation"
      ;;
    VALIDATED)
      if [ "$MODE" = rehearse ]; then
        drop
        echo "Rehearsed: Maven Central validated offline-protocol-android $VERSION, and the deployment was dropped."
        exit 0
      fi
      ;;
    PUBLISHING | PUBLISHED)
      [ "$MODE" = publish ] || die "a rehearsal deployment is $STATE: it was uploaded as $PUBLISHING_TYPE and should not publish"
      # Validated and released. Central takes a while longer to make it
      # readable; nothing after this point can stop it.
      echo "offline-protocol-android $VERSION is $STATE on Maven Central."
      exit 0
      ;;
    PENDING | VALIDATING) ;;
    *) die "unknown deployment state '$STATE': $(cat "$RESPONSE")" ;;
  esac
  sleep "$POLL_SECONDS"
done

die "deployment $DEPLOYMENT was still $STATE after $((POLLS * POLL_SECONDS)) seconds; check https://central.sonatype.com/publishing/deployments"
