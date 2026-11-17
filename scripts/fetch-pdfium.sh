#!/usr/bin/env bash
# Fetches the pdfium shared library into ~/.local/share/glossa/pdfium
# (the location Glossa looks at by default).
set -euo pipefail

dest="${GLOSSA_PDFIUM_DIR:-$HOME/.local/share/glossa/pdfium}"
tag="chromium/8125"

case "$(uname -m)" in
  x86_64)  asset="pdfium-linux-x64.tgz" ;;
  aarch64|arm64) asset="pdfium-linux-arm64.tgz" ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

url="https://github.com/bblanchon/pdfium-binaries/releases/download/${tag}/${asset}"

echo "Downloading ${url}"
mkdir -p "${dest}"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
curl -fL "${url}" -o "${tmp}/pdfium.tgz"
tar -xzf "${tmp}/pdfium.tgz" -C "${tmp}"

# the tarball carries lib/ at its root
cp -v "${tmp}/lib/libpdfium.so" "${dest}/libpdfium.so"
chmod +x "${dest}/libpdfium.so"

echo
echo "Installed: ${dest}/libpdfium.so"
echo "Glossa will find it automatically (or set GLOSSA_PDFIUM_LIB)."
