#!/usr/bin/env bash
# Builds a signed + notarised TrusCo Tracker DMG that opens on any Mac without warnings.
#
# One-time setup:
#   1. A "Developer ID Application" certificate in your login keychain
#      (Xcode → Settings → Accounts → Manage Certificates → + → Developer ID Application).
#   2. Notarisation credentials stored in the keychain (asks for an app-specific password
#      from appleid.apple.com → Sign-In and Security → App-Specific Passwords):
#        xcrun notarytool store-credentials trusco-notary --apple-id <your Apple ID> --team-id <your Team ID>
#
# Then:  ./scripts/release-mac.sh            (from the desktop folder; writes ../releases/*_universal.dmg)
#        ./scripts/release-mac.sh --dmg-only (redo only the DMG step, e.g. if the Mac locked mid-run:
#                                             the saved notary credentials can't be read while locked)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
PROFILE="${NOTARY_PROFILE:-trusco-notary}"

IDENTITY="${APPLE_SIGNING_IDENTITY:-$(security find-identity -v -p codesigning | sed -nE 's/.*"(Developer ID Application: [^"]+)".*/\1/p' | head -1)}"
if [[ -z "$IDENTITY" ]]; then
  echo "No 'Developer ID Application' certificate found in the keychain (see the setup notes at the top)." >&2
  exit 1
fi
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 || {
  echo "Notarisation profile '$PROFILE' not readable (is the Mac locked?) or not set up. Run: xcrun notarytool store-credentials $PROFILE --apple-id <Apple ID> --team-id <your Team ID>" >&2
  exit 1
}

VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
OUT="../releases"
# One universal build runs natively on Apple Silicon and Intel Macs.
TARGET="universal-apple-darwin"
APP="src-tauri/target/$TARGET/release/bundle/macos/TrusCo Tracker.app"
DMG="$OUT/TrusCo Tracker_${VERSION}_universal.dmg"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ "${1:-}" == "--dmg-only" ]]; then
  # Re-run just the packaging after an interrupted run (the app is already notarised).
  xcrun stapler validate "$APP" >/dev/null || { echo "The app isn't notarised yet; run without --dmg-only." >&2; exit 1; }
else
  echo "→ Building (Apple Silicon + Intel) and signing with: $IDENTITY"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  APPLE_SIGNING_IDENTITY="$IDENTITY" npx tauri build --bundles app --target "$TARGET"

  echo "→ Notarising the app"
  ditto -c -k --keepParent "$APP" "$WORK/app.zip"
  xcrun notarytool submit "$WORK/app.zip" --keychain-profile "$PROFILE" --wait
  xcrun stapler staple "$APP"
fi

echo "→ Packaging the DMG"
mkdir -p "$WORK/dmg" "$OUT"
cp -R "$APP" "$WORK/dmg/"
ln -s /Applications "$WORK/dmg/Applications"
rm -f "$DMG"
hdiutil create -volname "TrusCo Tracker" -srcfolder "$WORK/dmg" -ov -format UDZO "$DMG" >/dev/null
codesign --sign "$IDENTITY" --timestamp "$DMG"

echo "→ Notarising the DMG"
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$DMG"

echo "→ Gatekeeper check"
spctl --assess --type open --context context:primary-signature -v "$DMG"
spctl --assess --type execute -v "$APP"
# Fixed name so .../releases/latest/download/TrusCo-Tracker-mac.dmg always points at the newest version.
cp "$DMG" "$OUT/TrusCo-Tracker-mac.dmg"
echo "Done: $DMG"
echo "Attach to the GitHub release with:"
echo "  gh release upload v$VERSION \"$DMG\" \"$OUT/TrusCo-Tracker-mac.dmg\" --clobber"
