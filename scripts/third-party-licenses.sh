#!/bin/sh
# Regenerate THIRD-PARTY-LICENSES.md (licenses of all crates compiled into the
# streagit binary) with cargo-about, inside rust:alpine – no host toolchain.
#
# Usage: scripts/third-party-licenses.sh
# Config: about.toml (accepted licenses, targets), template: scripts/about.hbs
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
docker run --rm -v "$REPO:/src" -v cargo-cache:/usr/local/cargo/registry \
	-v streagit-tools:/tools -w /src rust:alpine sh -eu -c '
	apk add --no-cache musl-dev >/dev/null
	[ -x /tools/bin/cargo-about ] || cargo install --locked cargo-about --features cli --root /tools
	cargo fetch --locked
	/tools/bin/cargo-about generate --locked --offline --fail scripts/about.hbs -o THIRD-PARTY-LICENSES.md
'
