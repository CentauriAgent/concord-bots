#!/usr/bin/env bash
# =============================================================================
# deploy-nsite.sh — build the concord-bots docs and publish to Nostr (nsite)
# =============================================================================
#
# Flow: VitePress build (site/dist) → nsite-cli upload (kind 34128 events to
# Nostr relays + file blobs to Blossom servers). All content is signed by the
# publishing key; viewers/gateways assemble the site from relays + blossom.
#
# SAFETY INTERLOCK: this script refuses to publish unless NSITE_PUBLISH=yes is
# set. Without it, this is a dry run — it builds and prints the command.
#
# Required to publish:
#   NOSTR_PRIVATE_KEY   nsec (or hex) of the docs npub — which npub hosts the
#                       docs is the owner's decision; use a dedicated key.
#
# Optional overrides (defaults below):
#   NOSTR_RELAYS        comma-separated wss:// relays for the file-listing events
#   BLOSSOM_SERVERS     comma-separated https:// blob servers for the files
#
# Examples:
#   ./deploy-nsite.sh                       # dry run (build + print command)
#   NSITE_PUBLISH=yes \
#     NOSTR_PRIVATE_KEY=nsec1... \
#     ./deploy-nsite.sh                     # publish
# =============================================================================
set -euo pipefail

cd "$(dirname "$0")"

NOSTR_RELAYS="${NOSTR_RELAYS:-wss://nos.lol,wss://relay.primal.net,wss://relay.damus.io}"
BLOSSOM_SERVERS="${BLOSSOM_SERVERS:-https://nostr.download,https://cdn.satellite.earth}"
# nsite-cli is unmaintained upstream (successor: https://nsyte.run) — pinned
# for reproducibility. Bump deliberately after testing.
NSITE_CLI="nsite-cli@0.1.18"

echo "==> Building docs (VitePress)"
if [ ! -d node_modules ]; then
  echo "==> Installing site dependencies"
  npm install
fi
npm run docs:build

DIST_DIR="dist"
if [ ! -f "$DIST_DIR/index.html" ]; then
  echo "❌ Build failed — $DIST_DIR/index.html not found" >&2
  exit 1
fi
echo "==> Build OK: $DIST_DIR ($(find "$DIST_DIR" -type f | wc -l) files)"

if [ "${NSITE_PUBLISH:-}" != "yes" ]; then
  cat <<EOF

🔒 DRY RUN — not publishing (set NSITE_PUBLISH=yes to go live).

The publish command awaiting the owner's go:

  NSITE_PUBLISH=yes \\
    NOSTR_PRIVATE_KEY=nsec1YOUR_DOCS_NPUB_HERE \\
    NOSTR_RELAYS='$NOSTR_RELAYS' \\
    BLOSSOM_SERVERS='$BLOSSOM_SERVERS' \\
    npx $NSITE_CLI upload $DIST_DIR --fallback=/index.html

Notes:
  - The key becomes the site identity/address (/<npub>/ at every gateway).
    Use a dedicated docs npub — not the bot's nsec.
  - Once live, the site is served from relays + blossom by any nsite gateway,
    e.g. https://nsite.cc/<docs-npub>/
EOF
  exit 0
fi

if [ -z "${NOSTR_PRIVATE_KEY:-}" ]; then
  echo "❌ NSITE_PUBLISH=yes but NOSTR_PRIVATE_KEY is not set" >&2
  echo "   Export the docs npub's nsec: export NOSTR_PRIVATE_KEY=nsec1..." >&2
  exit 1
fi

echo "==> Publishing to Nostr (nsite)"
echo "    relays:   $NOSTR_RELAYS"
echo "    blossom:  $BLOSSOM_SERVERS"

# shellcheck disable=SC2086
npx -y "$NSITE_CLI" upload "$DIST_DIR" --fallback=/index.html

echo ""
echo "✅ Published. Site is live at nsite gateways under the publishing npub, e.g.:"
echo "   https://nsite.cc/<docs-npub>/"
echo "   (list files: npx $NSITE_CLI ls <docs-npub>)"
