#!/usr/bin/env bash
# Sign (with timestamp retries), notarize, staple, and wrap a DMG.
# Use when `npm run tauri build` fails on "timestamp service is not available"
# or hangs — after the .app already exists from a partial build.
#
#   source scripts/sign-env.sh && bash scripts/sign-notarize-dmg.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/src-tauri/target/release/bundle/macos/OpenPodcast.app"
ENT="$ROOT/src-tauri/Entitlements.plist"
ID="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Shivankar Sharma (NH6XX7B9AW)}"
DMG_DIR="$ROOT/src-tauri/target/release/bundle/dmg"
VERSION="$(node -p "require('$ROOT/package.json').version" 2>/dev/null || echo 0.1.1)"
ARCH="$(uname -m)"
case "$ARCH" in
  arm64) ARCH_LABEL=aarch64 ;;
  x86_64) ARCH_LABEL=x86_64 ;;
  *) ARCH_LABEL="$ARCH" ;;
esac
OUT_DMG="$DMG_DIR/OpenPodcast_${VERSION}_${ARCH_LABEL}.dmg"
MAX_TRIES="${CODESIGN_RETRIES:-12}"

if [[ ! -d "$APP" ]]; then
  echo "Missing app bundle: $APP"
  echo "Run a release build first (even if codesign fails mid-way)."
  exit 1
fi

if [[ -z "${APPLE_API_KEY_PATH:-}" || -z "${APPLE_API_KEY:-}" || -z "${APPLE_API_ISSUER:-}" ]]; then
  echo "Load notarization env first: source scripts/sign-env.sh"
  exit 1
fi

sign_retry() {
  local target=$1
  shift
  local n=0
  while (( n < MAX_TRIES )); do
    n=$((n + 1))
    echo "→ codesign ($n/$MAX_TRIES): $target"
    if codesign --force --options runtime --timestamp \
      --entitlements "$ENT" \
      --sign "$ID" \
      "$@" \
      "$target"; then
      return 0
    fi
    echo "  codesign failed (often Apple timestamp). Retrying in 5s…"
    sleep 5
  done
  echo "Gave up signing: $target"
  return 1
}

echo "Signing nested binaries…"
sign_retry "$APP/Contents/MacOS/ffmpeg"
sign_retry "$APP/Contents/MacOS/openpodcast"
echo "Signing app bundle…"
sign_retry "$APP"

echo "Verifying signature…"
codesign --verify --deep --strict --verbose=2 "$APP"
spctl --assess --type execute --verbose=4 "$APP" 2>&1 || true

WORK="$(mktemp -d)"
ZIP="$WORK/OpenPodcast.zip"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

echo "Zipping for notary…"
ditto -c -k --keepParent "$APP" "$ZIP"

echo "Submitting to Apple Notary (this can take several minutes)…"
xcrun notarytool submit "$ZIP" \
  --key "$APPLE_API_KEY_PATH" \
  --key-id "$APPLE_API_KEY" \
  --issuer "$APPLE_API_ISSUER" \
  --wait

echo "Stapling…"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"

echo "Building DMG…"
mkdir -p "$DMG_DIR"
rm -f "$OUT_DMG"
hdiutil create -volname OpenPodcast -srcfolder "$APP" -ov -format UDZO "$OUT_DMG"

echo "Done:"
echo "  $APP"
echo "  $OUT_DMG"
