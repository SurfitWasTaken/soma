#!/usr/bin/env bash
# Downloads a prebuilt PDFium shared library into vendor/pdfium/.
# Soma loads it at runtime from there (or from $PDFIUM_DYNAMIC_LIB_PATH).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/vendor/pdfium"
release="${PDFIUM_RELEASE:-latest}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  asset=pdfium-mac-arm64.tgz ;;
  Darwin-x86_64) asset=pdfium-mac-x64.tgz ;;
  Linux-x86_64)  asset=pdfium-linux-x64.tgz ;;
  Linux-aarch64) asset=pdfium-linux-arm64.tgz ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

if [[ "$release" == latest ]]; then
  url="https://github.com/bblanchon/pdfium-binaries/releases/latest/download/$asset"
else
  url="https://github.com/bblanchon/pdfium-binaries/releases/download/$release/$asset"
fi

mkdir -p "$dest"
echo "fetching $url"
curl -fsSL --retry 3 "$url" | tar -xz -C "$dest"
echo "installed: $(ls "$dest"/lib/libpdfium.*)"
