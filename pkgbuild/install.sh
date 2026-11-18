#!/usr/bin/env bash
# Marginal — install for the current user, the quick Omarchy-style way:
# release binary + desktop entry + icon into ~/.local, so the app shows up
# in the launcher right away. Update later with: cargo build --release && ./pkgbuild/install.sh
set -euo pipefail
cd "$(dirname "$0")/.."

command -v cargo >/dev/null || { echo "cargo not found — install the rust toolchain first"; exit 1; }
cargo build --release

install -Dm755 target/release/marginal "$HOME/.local/bin/marginal"
install -Dm644 pkgbuild/marginal.desktop "$HOME/.local/share/applications/marginal.desktop"
install -Dm644 pkgbuild/marginal.svg "$HOME/.local/share/icons/hicolor/scalable/apps/marginal.svg"

# refresh the launcher's app cache if the tool is around
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true

echo "installed:"
echo "  $HOME/.local/bin/marginal"
echo "  $HOME/.local/share/applications/marginal.desktop"
echo "  $HOME/.local/share/icons/hicolor/scalable/apps/marginal.svg"
echo "Marginal now opens from the app launcher (and 'marginal' is on your PATH)."
