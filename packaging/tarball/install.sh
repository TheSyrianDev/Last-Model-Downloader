#!/usr/bin/env sh
# Installs mdl (and the model-downloader alias) from this release folder.
# Usage: ./install.sh [PREFIX]   (default PREFIX: /usr/local; use ~/.local for no root)
set -eu
PREFIX="${1:-/usr/local}"
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$PREFIX/bin"
mkdir -p "$BIN"
install -m 755 "$HERE/mdl" "$BIN/mdl"
ln -sf mdl "$BIN/model-downloader"
"$BIN/mdl" --version
echo "Installed to $BIN (mdl, model-downloader). Make sure $BIN is on PATH."
