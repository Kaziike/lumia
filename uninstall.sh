#!/usr/bin/env bash
set -e

echo "=========================================================="
echo "          Uninstalling Lumina WM from your system"
echo "=========================================================="

# 1. Remove binaries and symlinks
CARGO_BIN="$HOME/.cargo/bin"
LOCAL_BIN="$HOME/.local/bin"

echo "-> Removing binaries and symlinks..."
rm -f "$CARGO_BIN/lumina-wm" "$CARGO_BIN/luminactl" "$CARGO_BIN/lumina-session"
rm -f "$LOCAL_BIN/lumina-wm" "$LOCAL_BIN/luminactl" "$LOCAL_BIN/lumina-session"

# 2. Remove configuration and assets
CONFIG_DIR="$HOME/.config/lumina"
if [ -d "$CONFIG_DIR" ]; then
    echo "-> Removing configuration directory: $CONFIG_DIR..."
    rm -rf "$CONFIG_DIR"
fi

# 3. Remove application icon
ICON_FILE="$HOME/.local/share/icons/hicolor/scalable/apps/lumina-wm.svg"
if [ -f "$ICON_FILE" ]; then
    echo "-> Removing application icon: $ICON_FILE..."
    rm -f "$ICON_FILE"
fi

# 4. Remove desktop entries
APP_FILE="$HOME/.local/share/applications/lumina-wm.desktop"
if [ -f "$APP_FILE" ]; then
    echo "-> Removing desktop launcher: $APP_FILE..."
    rm -f "$APP_FILE"
fi

# 5. Remove session files
WAYLAND_SESSION="$HOME/.local/share/wayland-sessions/lumina.desktop"
XSESSION="$HOME/.local/share/xsessions/lumina.desktop"

if [ -f "$WAYLAND_SESSION" ]; then
    echo "-> Removing Wayland session entry: $WAYLAND_SESSION..."
    rm -f "$WAYLAND_SESSION"
fi

if [ -f "$XSESSION" ]; then
    echo "-> Removing XSession entry: $XSESSION..."
    rm -f "$XSESSION"
fi

# 6. Update desktop application database
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
fi

echo "=========================================================="
echo "  Lumina WM uninstalled successfully!"
echo "=========================================================="
