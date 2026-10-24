#!/usr/bin/env bash
# Installs the app for the current user on macOS: an app bundle in ~/Applications, which Spotlight,
# Launchpad and the Dock find, and a short terminal command in ~/.local/bin. Every name comes from
# src/brand.rs (the binary prints them with --print-brand), so nothing is hardcoded here.
#
# The bundle registers no file types: macOS hands a file to an app in an Apple Event, which the
# window library (winit) can't receive, so "Open With" would only ever open scratch. Files open
# from the terminal command or from inside the app.
#
# `install-macos.sh --bundle DIR BINARY` only makes the app bundle in DIR from an already built
# BINARY and installs nothing; release.yml packages the macOS download that way.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname -s)" != Darwin ]; then
    echo "This installs the macOS app; on Linux run scripts/install.sh instead." >&2
    exit 1
fi

if [ "${1:-}" = --bundle ]; then
    apps_dir=$2
    binary=$3
else
    apps_dir="$HOME/Applications"
    cargo build --release
    # The binary's file name is Cargo.toml's package name.
    binary="target/release/$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
fi
eval "$("$binary" --print-brand)"
# The oldest macOS the build supports, e.g. MACOSX_DEPLOYMENT_TARGET=11.0 on Apple silicon.
eval "$(rustc --print deployment-target)"

# Inside the bundle the program is named for the app, so the menu bar reads "Quit <app>".
app="$apps_dir/$APP_NAME.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/$APP_NAME"
cp assets/icon/icon.icns "$app/Contents/Resources/icon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>$APP_ID</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MACOSX_DEPLOYMENT_TARGET</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# An ad-hoc signature ("-": no Apple developer account needed) seals Info.plist and the icon into
# the bundle, so macOS treats it as one app named by APP_ID. Each build signs differently, so after
# a reinstall the Keychain asks once before handing over the saved API key: choose Always Allow.
codesign --force --sign - "$app"

if [ "${1:-}" = --bundle ]; then
    exit 0
fi

# The terminal command runs the program inside the bundle (a symlink would lose the bundle).
bin_dir="$HOME/.local/bin"
mkdir -p "$bin_dir"
printf '#!/bin/sh\nexec "%s" "$@"\n' "$app/Contents/MacOS/$APP_NAME" > "$bin_dir/$BIN_NAME"
chmod 755 "$bin_dir/$BIN_NAME"

echo "Installed $APP_NAME as $app"
echo "Settings and scratch live in ~/Library/Application Support/$CONFIG_DIR_NAME"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Note: $bin_dir is not on your PATH, so add it to run '$BIN_NAME' from a terminal." ;;
esac
