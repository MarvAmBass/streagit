#!/bin/sh
# Compare streagit per-page latency with a full stagit render on a real repository.
#
# Usage: scripts/bench.sh <path-to-bare-repo.git> [path-to-stagit-source]
#
# Runs in rust:alpine (Docker): builds streagit --release, builds stagit 1.3
# (clone or the given source), copies the repo into the container's own file
# system (so both sides read the same disk), starts streagit and times a set of
# pages cold and warm with curl, then times `stagit` writing every page.
set -eu
REPO_PATH=$(cd "$1" && pwd)
HERE=$(cd "$(dirname "$0")/.." && pwd)
SRC=${2:-}
MOUNT=""
if [ -n "$SRC" ]; then
	MOUNT="-v $(cd "$SRC" && pwd):/stagit-src:ro"
fi
# shellcheck disable=SC2086
exec docker run --rm -v "$HERE:/src" -v "$REPO_PATH:/bench-repo:ro" $MOUNT \
	-v cargo-cache:/usr/local/cargo/registry -v streagit-target:/src/target -w /src rust:alpine sh -eu -c '
apk add --no-cache musl-dev build-base libgit2-dev git curl >/dev/null
echo "== building streagit (release)"
cargo build --release --quiet 2>&1 | tail -3 || true
ls -la target/release/streagit
echo "== building stagit 1.3"
if [ -d /stagit-src ]; then cp -r /stagit-src /tmp/stagit; else git clone -q git://git.codemadness.org/stagit /tmp/stagit; fi
(cd /tmp/stagit && git checkout -q 1.3 && make clean >/dev/null && make >/dev/null)
echo "== copying repository into the container"
SLOT=/tmp/bench/root/github.com/bench/repos/repo
mkdir -p "$SLOT"
cp -r /bench-repo "$SLOT/repo.git"
cd "$SLOT/repo.git"
git count-objects -vH | sed "s/^/   /"
N=$(git rev-list --count HEAD)
echo "   commits: $N, files in HEAD: $(git ls-tree -r HEAD | wc -l), tags: $(git tag | wc -l)"
# the three commits with the most changed lines (git numstat) among those that
# stagit would not suppress by file count; libgit2 counts exact renames only, so
# some of them may still end up as "Diff is too large"
BIGS=$(git log --format="%H" --shortstat HEAD | awk "/^[0-9a-f]{40}\$/ {h=\$1} /changed/ {f=\$1; a=0; d=0; for(i=2;i<=NF;i++) { if (\$i ~ /insertion/) a=\$(i-1); if (\$i ~ /deletion/) d=\$(i-1) } if (f<=1000) print a+d, h}" | sort -rn | head -3 | cut -d" " -f2)
NEXT=$(git rev-list HEAD | sed -n 101p)
README=$(git ls-tree --name-only HEAD | grep -i "^readme" | head -1 || true)
echo "   big commits: $(echo $BIGS)"
echo "== starting streagit"
/src/target/release/streagit --listen 127.0.0.1:8081 --root /tmp/bench/root --prefix /browse --page-size 100 --log-level warn &
PID=$!
sleep 0.5
curl -sf -o /dev/null http://127.0.0.1:8081/browse/healthz
B=http://127.0.0.1:8081/browse/github.com/bench/repos/repo/html
t() { # label url [extra curl args]
	label=$1; url=$2; shift 2
	out=$(curl -s -o /tmp/body -w "%{http_code} %{time_total} %{size_download}" "$@" "$url")
	printf "   %-44s status %s  %6.3f s  %9s bytes\n" "$label" $(echo "$out" | cut -d" " -f1) $(echo "$out" | cut -d" " -f2) $(echo "$out" | cut -d" " -f3)
}
echo "== streagit page latencies (first = cold, second = warm/cached)"
t "log.html (100 rows, 100 diffs)" "$B/log.html"
t "log.html again (render cache)" "$B/log.html"
ETAG=$(curl -sI "$B/log.html" | awk "tolower(\$1)==\"etag:\" {print \$2}" | tr -d "\r")
t "log.html If-None-Match (304)" "$B/log.html" -H "If-None-Match: $ETAG"
t "log.html?from=<101st> (page 2 cold)" "$B/log.html?from=$NEXT"
t "commit/<HEAD>.html" "$B/commit/$(git rev-parse HEAD).html"
for BIG in $BIGS; do
	t "commit/${BIG%????????????????????????????????}.html cold" "$B/commit/$BIG.html"
	t "commit/${BIG%????????????????????????????????}.html again" "$B/commit/$BIG.html"
	t "commit/${BIG%????????????????????????????????}.html gzip" "$B/commit/$BIG.html" -H "Accept-Encoding: gzip"
done
t "files.html cold" "$B/files.html"
t "files.html again" "$B/files.html"
if [ -n "$README" ]; then t "file/$README.html" "$B/file/$README.html"; fi
t "refs.html cold" "$B/refs.html"
t "refs.html again" "$B/refs.html"
t "atom.xml" "$B/atom.xml"
t "tags.xml" "$B/tags.xml"
echo "== streagit: whole history through log pages (diffstat for every commit, once)"
start=$(date +%s.%N)
url="$B/log.html"; pages=0
while [ -n "$url" ]; do
	pages=$((pages+1))
	next=$(curl -s "$url" | grep -o "log.html?from=[0-9a-f]*" | head -1 || true)
	if [ -n "$next" ]; then url="$B/$next"; else url=""; fi
done
end=$(date +%s.%N)
printf "   %s pages, %.2f s total\n" "$pages" "$(echo "$end - $start" | bc)"
echo "== streagit memory"
grep -E "VmRSS|VmHWM" /proc/$PID/status | sed "s/^/   /"
kill $PID
echo "== stagit full render (cold, no cache; writes every page)"
mkdir -p /tmp/bench/out && cd /tmp/bench/out
start=$(date +%s.%N)
/tmp/stagit/stagit "$SLOT/repo.git"
end=$(date +%s.%N)
printf "   %.2f s, %s files, %s\n" "$(echo "$end - $start" | bc)" "$(find . -type f | wc -l)" "$(du -sh . | cut -f1)"
'
