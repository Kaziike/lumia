#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_SRC_DIR="$SCRIPT_DIR/target/release"

echo "=========================================================="
echo "          Installing Lumina WM to your system"
echo "=========================================================="

# 1. Check binaries
if [ ! -f "$BIN_SRC_DIR/lumina-wm" ] || [ ! -f "$BIN_SRC_DIR/luminactl" ]; then
    echo "Release binaries not found in target/release/."
    echo "Attempting to build with cargo..."
    cargo build --release --bins
fi

# 2. Setup user binary destinations
CARGO_BIN="$HOME/.cargo/bin"
LOCAL_BIN="$HOME/.local/bin"
mkdir -p "$CARGO_BIN" "$LOCAL_BIN"

echo "-> Installing binaries to $CARGO_BIN and $LOCAL_BIN..."
cp -f "$BIN_SRC_DIR/lumina-wm" "$CARGO_BIN/lumina-wm"
cp -f "$BIN_SRC_DIR/luminactl" "$CARGO_BIN/luminactl"
chmod +x "$CARGO_BIN/lumina-wm" "$CARGO_BIN/luminactl"

# Symlink or copy to ~/.local/bin
ln -sf "$CARGO_BIN/lumina-wm" "$LOCAL_BIN/lumina-wm"
ln -sf "$CARGO_BIN/luminactl" "$LOCAL_BIN/luminactl"

# 3. Create helper session launcher: lumina-session
cat << 'EOF' > "$CARGO_BIN/lumina-session"
#!/usr/bin/env bash
set -e

# Ensure cargo and local user bins are in PATH
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

BIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LUMINA_BIN="$BIN_DIR/lumina-wm"
if [ ! -x "$LUMINA_BIN" ]; then
    LUMINA_BIN="$(command -v lumina-wm || true)"
fi

ASSETS_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/lumina/assets"
if [ ! -d "$ASSETS_DIR" ]; then
    ASSETS_DIR="$HOME/.config/lumina/assets"
fi

export XDG_CURRENT_DESKTOP="${XDG_CURRENT_DESKTOP:-Lumina}"
export XDG_SESSION_DESKTOP="${XDG_SESSION_DESKTOP:-lumina}"

# When launched from a Display Manager (GDM/SDDM/LightDM) or TTY without an existing display server:
if [ -z "$WAYLAND_DISPLAY" ] && [ -z "$DISPLAY" ] && [ -z "$LUMINA_IN_CAGE" ]; then
    if command -v cage >/dev/null 2>&1; then
        export LUMINA_IN_CAGE=1
        exec cage -s -d -- "$0" "$@"
    else
        echo "Error: No display server found (neither WAYLAND_DISPLAY nor DISPLAY is set) and 'cage' is not installed." >&2
        echo "Please install cage: sudo pacman -S cage" >&2
        exit 1
    fi
fi

# Run Lumina WM compositor
if [ -d "$ASSETS_DIR" ]; then
    exec "$LUMINA_BIN" --assets "$ASSETS_DIR" "$@"
else
    exec "$LUMINA_BIN" "$@"
fi
EOF
chmod +x "$CARGO_BIN/lumina-session"
ln -sf "$CARGO_BIN/lumina-session" "$LOCAL_BIN/lumina-session"

# 4. Install assets & configuration to ~/.config/lumina/assets
CONFIG_ASSETS_DIR="$HOME/.config/lumina/assets"
echo "-> Installing HTML/CSS shell assets to $CONFIG_ASSETS_DIR..."
mkdir -p "$CONFIG_ASSETS_DIR"
cp -r "$SCRIPT_DIR/assets/"* "$CONFIG_ASSETS_DIR/"

# 5. Install App Icon
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$ICON_DIR"
echo "-> Installing application icon..."
cat << 'EOF' > "$ICON_DIR/lumina-wm.svg"
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
  <defs>
    <linearGradient id="luminaBg" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#1e1e2e"/>
      <stop offset="100%" stop-color="#11111b"/>
    </linearGradient>
    <linearGradient id="luminaGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#cba6f7"/>
      <stop offset="50%" stop-color="#89b4fa"/>
      <stop offset="100%" stop-color="#f38ba8"/>
    </linearGradient>
  </defs>
  <rect width="128" height="128" rx="28" fill="url(#luminaBg)" stroke="#313244" stroke-width="2"/>
  <polygon points="64,22 104,46 104,82 64,106 24,82 24,46" fill="none" stroke="url(#luminaGrad)" stroke-width="7" stroke-linejoin="round"/>
  <polygon points="64,36 90,52 90,76 64,92 38,76 38,52" fill="url(#luminaGrad)" opacity="0.18"/>
  <circle cx="64" cy="64" r="12" fill="url(#luminaGrad)"/>
</svg>
EOF

# 6. Install Desktop App Entry (for application menu in Cinnamon / GNOME / etc.)
APP_DIR="$HOME/.local/share/applications"
mkdir -p "$APP_DIR"
echo "-> Installing desktop launcher to $APP_DIR/lumina-wm.desktop..."
cat << EOF > "$APP_DIR/lumina-wm.desktop"
[Desktop Entry]
Version=1.0
Type=Application
Name=Lumina WM
GenericName=Wayland Window Manager
Comment=Pure-Rust Wayland Window Manager & HTML/CSS Desktop Shell
Exec=$CARGO_BIN/lumina-session
Icon=lumina-wm
Terminal=false
Categories=System;Utility;
Keywords=wayland;window;manager;desktop;shell;rust;
StartupNotify=false
EOF

# 7. Install Session Entries (for Display Manager login)
SESSION_DIR="$HOME/.local/share/wayland-sessions"
XSESSION_DIR="$HOME/.local/share/xsessions"
mkdir -p "$SESSION_DIR" "$XSESSION_DIR"
echo "-> Installing session entries to $SESSION_DIR and $XSESSION_DIR..."
cat << EOF > "$SESSION_DIR/lumina.desktop"
[Desktop Entry]
Name=Lumina WM (Wayland)
Comment=Pure-Rust Wayland Window Manager & HTML/CSS Desktop Shell
Exec=$CARGO_BIN/lumina-session
Type=Application
DesktopNames=Lumina
EOF

cat << EOF > "$XSESSION_DIR/lumina.desktop"
[Desktop Entry]
Name=Lumina WM
Comment=Pure-Rust Wayland Window Manager & HTML/CSS Desktop Shell
Exec=$CARGO_BIN/lumina-session
Type=Application
DesktopNames=Lumina
EOF

# Update desktop application database if available
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APP_DIR" 2>/dev/null || true
fi

echo "=========================================================="
echo "  Installation completed successfully!"
echo "=========================================================="
echo "Installed components:"
echo "  - Compositor:   $CARGO_BIN/lumina-wm"
echo "  - CLI tool:     $CARGO_BIN/luminactl"
echo "  - Session:      $CARGO_BIN/lumina-session"
echo "  - Shell Assets: $CONFIG_ASSETS_DIR"
echo "  - App Launcher: $APP_DIR/lumina-wm.desktop"
echo "  - Session Files: $XSESSION_DIR/lumina.desktop, $SESSION_DIR/lumina.desktop"
echo ""
echo "How to run:"
echo "  1. Directly in terminal:  lumina-wm  or  lumina-session"
echo "  2. From Application Menu: Search 'Lumina WM'"
echo "  3. CLI management:        luminactl --help"
echo "  4. To enable in LightDM login screen:"
echo "     sudo cp ~/.local/share/xsessions/lumina.desktop /usr/share/xsessions/"
echo "=========================================================="

