#!/usr/bin/env bash
# Builds the TrusCo Tasks widget extension (universal) into src-tauri/widget-build/TrusCoTasksWidget.appex.
# Tauri copies it into "TrusCo Tracker.app/Contents/PlugIns" (see src-tauri/tauri.macos.conf.json).
# Signs with $APPLE_SIGNING_IDENTITY (Developer ID, for releases) or ad hoc for local builds.
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="src-tauri/widget-build"
APPEX="$OUT/TrusCoTasksWidget.appex"
VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
SDK="$(xcrun --sdk macosx --show-sdk-path)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# App extensions must start at _NSExtensionMain (as Xcode links them): it sets up the extension
# context and then runs the @main WidgetBundle. Starting at plain main() makes WidgetKit's
# bootstrap trap ("ExtensionMain.shared" is never configured) and the widget can't be added.
for arch in arm64 x86_64; do
  xcrun swiftc -sdk "$SDK" -target "$arch-apple-macos14.0" -parse-as-library -application-extension -O \
    -module-name TrusCoTasksWidget widget/TasksWidget.swift -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$WORK/TrusCoTasksWidget-$arch"
done

rm -rf "$APPEX"
mkdir -p "$APPEX/Contents/MacOS"
lipo -create "$WORK"/TrusCoTasksWidget-arm64 "$WORK"/TrusCoTasksWidget-x86_64 -output "$APPEX/Contents/MacOS/TrusCoTasksWidget"
sed "s/__VERSION__/$VERSION/g" widget/Info.plist > "$APPEX/Contents/Info.plist"

IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
if [[ "$IDENTITY" == "-" ]]; then
  codesign --force --sign - --entitlements widget/TasksWidget.entitlements "$APPEX"
else
  codesign --force --sign "$IDENTITY" --options runtime --timestamp --entitlements widget/TasksWidget.entitlements "$APPEX"
fi
nm -u "$APPEX/Contents/MacOS/TrusCoTasksWidget" | grep -q _NSExtensionMain || { echo "widget entry point is not _NSExtensionMain" >&2; exit 1; }
echo "Built $APPEX ($(lipo -archs "$APPEX/Contents/MacOS/TrusCoTasksWidget"), signed: $IDENTITY)"
