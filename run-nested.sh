#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

echo "=========================================================="
echo " Starting Lumina WM in Nested Mode (Wayland / X11 Session)"
echo "=========================================================="
echo "IPC Socket: ${XDG_RUNTIME_DIR:-/tmp}/lumina-wm.sock"
echo ""
echo "Quick Keys in Nested Window:"
echo "  - Click 'Lumina' or press [Super] / [Esc] -> Toggle App Launcher"
echo "  - Click [1..9] Workspace pills -> Switch workspace"
echo "  - Click [Layout] badge -> Cycle layout (Tiling / Scrolling / BSP)"
echo "  - Click [Audio / Clock] -> Open Quick Settings / Control Center"
echo "  - Click Window titlebar [O] [□] [X] -> Float / Maximize / Close"
echo ""
echo "To control from another terminal:"
echo "  ./target/debug/luminactl wm workspaces"
echo "  ./target/debug/luminactl dispatch \"wm:set-layout-mode(scrolling)\""
echo "  ./target/debug/luminactl css set-var --accent \"#f38ba8\""
echo "=========================================================="

cargo build --bins
exec ./target/debug/lumina-wm "$@"
