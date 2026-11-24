#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET=""
ARCH_LABEL=""
MAKE_PACKAGES=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --arch-label)
            ARCH_LABEL="$2"
            shift 2
            ;;
        --package)
            MAKE_PACKAGES=true
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')

if [[ -n "$TARGET" ]]; then
    cargo build --release --target "$TARGET"
    BIN_PATH="target/$TARGET/release/ai-switcher"
    OUT_BASE="target/$TARGET/release/bundle"
else
    cargo build --release
    BIN_PATH="target/release/ai-switcher"
    OUT_BASE="target/release/bundle"
fi

if [[ -z "$ARCH_LABEL" ]]; then
    case "${TARGET:-$(uname -m)}" in
        aarch64-apple-darwin|arm64) ARCH_LABEL="arm64" ;;
        x86_64-apple-darwin|x86_64) ARCH_LABEL="x86_64" ;;
        *) ARCH_LABEL="$(uname -m)" ;;
    esac
fi

APP_NAME="AI Switcher.app"
BUNDLE_DIR="$OUT_BASE/$APP_NAME"
rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/Contents/MacOS" "$BUNDLE_DIR/Contents/Resources"

cp "$BIN_PATH" "$BUNDLE_DIR/Contents/MacOS/ai-switcher"
if [ -f assets/AppIcon.icns ]; then
    cp assets/AppIcon.icns "$BUNDLE_DIR/Contents/Resources/AppIcon.icns"
fi

cat > "$BUNDLE_DIR/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>AI Switcher</string>
    <key>CFBundleDisplayName</key>
    <string>AI Switcher</string>
    <key>CFBundleIdentifier</key>
    <string>com.xwyin.ai-switcher</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleExecutable</key>
    <string>ai-switcher</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

codesign --force --deep --sign - "$BUNDLE_DIR" >/dev/null 2>&1 || true
touch "$BUNDLE_DIR"
echo "Built: $BUNDLE_DIR"

if [[ "$MAKE_PACKAGES" == true ]]; then
    DIST_DIR="target/dist"
    mkdir -p "$DIST_DIR"

    ZIP_NAME="AI-Switcher-v${VERSION}-macos-${ARCH_LABEL}.zip"
    DMG_NAME="AI-Switcher-v${VERSION}-macos-${ARCH_LABEL}.dmg"

    ditto -c -k --sequesterRsrc --keepParent "$BUNDLE_DIR" "$DIST_DIR/$ZIP_NAME"

    DMG_STAGING="$OUT_BASE/dmg-staging"
    rm -rf "$DMG_STAGING"
    mkdir -p "$DMG_STAGING"
    cp -R "$BUNDLE_DIR" "$DMG_STAGING/"
    ln -s /Applications "$DMG_STAGING/Applications"
    hdiutil create -volname "AI Switcher" -srcfolder "$DMG_STAGING" -ov -format UDZO "$DIST_DIR/$DMG_NAME" >/dev/null
    rm -rf "$DMG_STAGING"

    echo "Packaged: $DIST_DIR/$DMG_NAME"
    echo "Packaged: $DIST_DIR/$ZIP_NAME"
fi
