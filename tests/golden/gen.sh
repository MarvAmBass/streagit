#!/bin/sh
# Regenerate the golden stagit 1.3 output for the fixture (SPEC §12.2).
#
# Builds stagit 1.3 against libgit2 in alpine:3, builds the fixture with
# tests/fixtures/make_fixture.sh and runs stagit on the main fixture repo.
# Output: tests/golden/fixture/{log.html,files.html,refs.html,atom.xml,tags.xml,commit/*,file/*}
#
# Usage: tests/golden/gen.sh [path-to-stagit-source]   (default: git clone git://git.codemadness.org/stagit)
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
SRC=${1:-}
MOUNT=""
if [ -n "$SRC" ]; then
	MOUNT="-v $(cd "$SRC" && pwd):/stagit-src:ro"
fi
rm -rf "$HERE/fixture"
mkdir -p "$HERE/fixture"
# shellcheck disable=SC2086
docker run --rm -v "$REPO:/src" $MOUNT -w /src alpine:3 sh -eu -c '
	apk add --no-cache build-base libgit2-dev git >/dev/null
	if [ -d /stagit-src ]; then cp -r /stagit-src /tmp/stagit; else git clone -q git://git.codemadness.org/stagit /tmp/stagit; fi
	cd /tmp/stagit && git checkout -q 1.3 && make clean >/dev/null && make >/dev/null
	sh /src/tests/fixtures/make_fixture.sh /tmp/fx/root >/dev/null
	mkdir -p /tmp/golden && cd /tmp/golden
	/tmp/stagit/stagit /tmp/fx/root/github.com/alice/repos/fixture/repo.git
	# keep the golden set small: drop the 10k-line blob page; the page for the
	# non-UTF-8 file name cannot be written onto every host filesystem
	rm -f file/big.txt.html "file/$(printf "caf\351")".txt.html
	cp -a /tmp/golden/. /src/tests/golden/fixture/
	echo "stagit version: $(cd /tmp/stagit && git describe --tags)"
	echo "libgit2: $(apk info libgit2 2>/dev/null | head -1)"
'
echo "golden files written to $HERE/fixture"
