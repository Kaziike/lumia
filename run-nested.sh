#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

echo "=========================================================="
echo " Starting Lumina WM in Nested Mode (Release / Zero-Lag)"
echo "=========================================================="
echo "IPC Socket: ${XDG_RUNTIME_DIR:-/tmp}/lumina-wm.sock"
echo ""
echo "Interactive Controls in Nested Window:"
echo "  - Click 'Lumina' or press [Super] / [Esc] -> Toggle App Launcher"
echo "  - Click [1..9] or press [F1..F4]          -> Switch Workspace"
echo "  - Click [Layout] badge                    -> Cycle Layout (Tiling / Scrolling / BSP / Monocle / Floating)"
echo "  - Click [Audio / Clock / Bell]            -> Toggle Quick Settings & Notifications"
echo "  - Click Dock icons [Term / Web / Files / Code] -> Launch Application Window"
echo "  - Click Window Titlebar [O] [□] [X]       -> Float / Maximize / Close Window"
echo "  - Type commands & press [Enter]           -> Execute live shell commands inside Terminal window"
echo ""
echo "To control from another terminal:"
echo "  ./target/release/luminactl wm workspaces"
echo "  ./target/release/luminactl dispatch \"wm:set-layout-mode(scrolling)\""
echo "  ./target/release/luminactl css set-var --accent \"#f38ba8\""
echo "=========================================================="

cargo build --release --bins
exec ./target/release/lumina-wm "$@"
