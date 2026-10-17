#!/usr/bin/env bash
# Installs the app for the current user: the release binary in ~/.local/bin, the app icon, a
# .desktop entry for app launchers, and a MIME type so project files open in the app. Every name comes from
# src/brand.rs (the binary prints them with --print-brand), so nothing is hardcoded here.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release
# The binary's file name is Cargo.toml's package name.
binary="target/release/$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
eval "$("$binary" --print-brand)"

bin_dir="$HOME/.local/bin"
apps_dir="$HOME/.local/share/applications"
mime_dir="$HOME/.local/share/mime"
icons_dir="$HOME/.local/share/icons/hicolor"
mkdir -p "$bin_dir" "$apps_dir" "$mime_dir/packages"

install -m 755 "$binary" "$bin_dir/$BIN_NAME"

# The icon files are named generically; they are installed under the app id so launchers find them.
install -D -m 644 assets/icon/icon.svg "$icons_dir/scalable/apps/$APP_ID.svg"
for size in 16 24 32; do
    install -D -m 644 assets/icon/icon-small.svg "$icons_dir/${size}x${size}/apps/$APP_ID.svg"
done
install -D -m 644 assets/icon/icon-symbolic.svg "$icons_dir/symbolic/apps/$APP_ID-symbolic.svg"

cat > "$apps_dir/$APP_ID.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$APP_NAME
Comment=Plain-text editor and scratchpad
Exec=$bin_dir/$BIN_NAME %f
Icon=$APP_ID
Terminal=false
Categories=Utility;TextEditor;
MimeType=text/plain;$PROJECT_MIME;
StartupWMClass=$APP_ID
DESKTOP

cat > "$mime_dir/packages/$APP_ID.xml" <<MIME
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="$PROJECT_MIME">
    <comment>$APP_NAME project</comment>
    <icon name="$APP_ID"/>
    <glob pattern="*.$PROJECT_EXT"/>
  </mime-type>
</mime-info>
MIME

# Refresh the launcher and MIME caches when the tools are there; both are optional.
if command -v update-mime-database > /dev/null; then
    update-mime-database "$mime_dir"
fi
if command -v update-desktop-database > /dev/null; then
    update-desktop-database "$apps_dir"
fi
if command -v gtk-update-icon-cache > /dev/null && [ -f "$icons_dir/index.theme" ]; then
    gtk-update-icon-cache -q "$icons_dir"
fi

echo "Installed $APP_NAME as $bin_dir/$BIN_NAME"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Note: $bin_dir is not on your PATH, so add it to run '$BIN_NAME' from a terminal." ;;
esac
