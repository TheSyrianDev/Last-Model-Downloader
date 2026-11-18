#!/usr/bin/env bash
# Tests a local release tarball in a clean container: install, version, and a tiny download.
# Usage: container-test.sh <image> <tarball>
set -euo pipefail
IMAGE="$1"; TAR="$(readlink -f "$2")"
docker run --rm -v "$TAR:/pkg/model-downloader.tar.gz:ro" "$IMAGE" sh -c '
set -eu
if command -v pacman >/dev/null; then pacman -Syu --noconfirm >/dev/null 2>&1 && pacman -S --noconfirm tar ca-certificates >/dev/null 2>&1; fi
if command -v apt-get >/dev/null; then apt-get update >/dev/null 2>&1 && apt-get install -y -qq ca-certificates >/dev/null 2>&1; fi
cd /tmp && tar xzf /pkg/model-downloader.tar.gz && cd model-downloader-*-linux-x86_64
(cd . && sha256sum -c SHA256SUMS.txt >/dev/null)
./install.sh /root/.local
export PATH=/root/.local/bin:$PATH
mdl --version
model-downloader --version
mdl download hf-internal-testing/tiny-random-bert config.json --local-dir /tmp/out --flat --format quiet
test -s /tmp/out/config.json && echo "DOWNLOAD OK"
'
