#!/usr/bin/env bash
# Build Legwork.app, a double-clickable Apple Silicon macOS bundle.
#
# Usage: scripts/macos/bundle-macos.sh
# Output: dist/Legwork.app  (and a zip alongside it)
set -euo pipefail

APP_NAME="Legwork"
BUNDLE_ID="dev.legwork.app"
BIN="legwork"
TARGET="aarch64-apple-darwin"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"

echo "==> Building release binary ($TARGET)"
cargo build --release --target "$TARGET"

echo "==> Assembling $APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/$BIN" "$APP/Contents/MacOS/$APP_NAME"
chmod +x "$APP/Contents/MacOS/$APP_NAME"

# Icon: build the .icns from the project-root app_icon.png (a 1024px master). We
# emit the full standard iconset (every size + @2x variant iconutil recognizes)
# so the Dock, Finder and launch animation each find the representation they
# want; missing the large ones makes the Dock fall back to a generic icon.
# Skipped entirely if the source or sips/iconutil are missing.
ICON_SRC="$ROOT/app_icon.png"

ICON_LINE=""
if [[ -f "$ICON_SRC" ]] && command -v sips >/dev/null && command -v iconutil >/dev/null; then
  echo "==> Building icon from $(basename "$ICON_SRC")"
  TMP="$(mktemp -d)"
  ICONSET="$TMP/$APP_NAME.iconset"
  mkdir -p "$ICONSET"
  # name:pixels for each entry iconutil expects (icon_<pt>x<pt>[@2x]).
  for spec in \
    icon_16x16:16     icon_16x16@2x:32 \
    icon_32x32:32     icon_32x32@2x:64 \
    icon_128x128:128  icon_128x128@2x:256 \
    icon_256x256:256  icon_256x256@2x:512 \
    icon_512x512:512  icon_512x512@2x:1024; do
    name="${spec%%:*}"; px="${spec##*:}"
    sips -s format png -z "$px" "$px" "$ICON_SRC" --out "$ICONSET/$name.png" >/dev/null
  done
  if iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/$APP_NAME.icns" 2>/dev/null; then
    ICON_LINE="<key>CFBundleIconFile</key><string>$APP_NAME</string>"
  else
    echo "   (icon generation failed; bundling without an icon)"
  fi
  rm -rf "$TMP"
else
  echo "==> app_icon.png or icon tools not found; bundling without an icon"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleExecutable</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    $ICON_LINE
</dict>
</plist>
PLIST

echo "==> Ad-hoc code signing"
codesign --force --deep --sign - "$APP" 2>/dev/null || echo "   (codesign unavailable; skipping)"

# Refresh the icon: macOS caches icons per bundle path, so rebuilding at the same
# path can keep showing a stale (or blank) icon. Bump the mtime and re-register the
# bundle with LaunchServices so Finder/Dock pick up the new icon on next launch.
echo "==> Refreshing icon cache"
touch "$APP"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
[[ -x "$LSREGISTER" ]] && "$LSREGISTER" -f "$APP" || true

echo "==> Zipping"
( cd "$DIST" && rm -f "$APP_NAME-$VERSION-$TARGET.zip" \
  && /usr/bin/ditto -c -k --keepParent "$APP_NAME.app" "$APP_NAME-$VERSION-$TARGET.zip" )

echo "Done: $APP"
echo "      $DIST/$APP_NAME-$VERSION-$TARGET.zip"
