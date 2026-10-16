#!/bin/sh
# Build the deterministic fixture data root used by the tests and the golden
# generation. Usage: make_fixture.sh <root>
#
# Layout (gitbackup-style):
#   github.com/alice/repos/fixture/repo.git         main repo (~35 commits, see below)
#   github.com/alice/repos/fixture/repo.v1.git      frozen older state, relative alternates
#   github.com/alice/repos/fixture/wiki.git         small wiki repo
#   github.com/alice/repos/fixture/forks/bob/repo.git        fork with own commit, alternates -> parent
#   github.com/alice/repos/fixture/forks/bob/repo.v1.git     fork version, alternates -> fork (nested)
#   github.com/alice/repos/html/repo.git            a repo literally named "html"
#   github.com/alice/repos/empty/repo.git           no commits at all
#   github.com/alice/gists/abc123/repo.git          gist
#   github.com/alice/repos/paging/repo.git          merge topology that breaks naive ?from= paging
#   github.com/carol/stars/alice/fixture            symlink -> ../../../alice/repos/fixture
#   github.com/alice/repos/evil/repo.git            symlink escaping the root (must be refused)
set -eu
mkdir -p "$1"
ROOT=$(cd "$1" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

export GIT_AUTHOR_NAME="Alice Example" GIT_AUTHOR_EMAIL="alice@example.com"
export GIT_COMMITTER_NAME="Alice Example" GIT_COMMITTER_EMAIL="alice@example.com"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
T=1700000000
# commit with the next deterministic timestamp; $1 = message, rest = extra git commit args
c() {
	T=$((T + 3600))
	GIT_AUTHOR_DATE="@$T +0100" GIT_COMMITTER_DATE="@$T +0100" git commit -q --allow-empty-message "$@"
}
git_init() {
	git init -q -b main "$1"
}
meta() { # dir description url owner
	printf '%s\n' "$2" > "$1/description"
	printf '%s\n' "$3" > "$1/url"
	printf '%s\n' "$4" > "$1/owner"
}

# ---------------------------------------------------------------- main repo
git_init "$WORK/main"
cd "$WORK/main"
git config core.autocrlf false

printf '# Fixture\n\nA <test> repository & "quotes".\n' > README.md
printf 'MIT License\n\nCopyright (c) 2024 Alice\n' > LICENSE
git add README.md LICENSE
c -m "Initial commit"

mkdir -p src docs
cat > src/main.c <<'EOF'
#include <stdio.h>

int main(void)
{
	printf("hello\n");
	return 0;
}
EOF
git add src/main.c
c -m "Add main.c"

cat > src/util.c <<'EOF'
int add(int a, int b)
{
	return a + b;
}

int sub(int a, int b)
{
	return a - b;
}
EOF
git add src/util.c
c -m "Add util.c

With a longer body that spans
multiple lines.

And a second paragraph."

sed -i.bak 's/hello/hello world/' src/main.c && rm src/main.c.bak
git add src/main.c
c -m "Say hello world"

# file without trailing newline, then fix it
printf 'line one\nline two' > nonl.txt
git add nonl.txt
c -m "Add file without trailing newline"
printf 'line one\nline two\n' > nonl.txt
git add nonl.txt
c -m "Add trailing newline"

# CRLF file
printf 'a\r\nb\r\nc\r\n' > crlf.txt
git add crlf.txt
c -m "Add CRLF file"

# empty file
: > empty.txt
git add empty.txt
c -m "Add empty file"

# binary add + modify
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x01' > image.png
git add image.png
c -m "Add binary file"
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x02\x00\xff' > image.png
git add image.png
c -m "Modify binary file"

# 10k line file
i=1; while [ $i -le 10000 ]; do echo "line $i"; i=$((i + 1)); done > big.txt
git add big.txt
c -m "Add big file"

# exact rename
git mv src/util.c src/math.c
c -m "Rename util.c to math.c"

# exact copy of a modified file: libgit2 (FIND_COPIES, exact match) reports an
# addition as a copy when it equals the *old* content of a file modified in the
# same commit.
cp src/main.c src/main_copy.c
printf '\n/* trailer */\n' >> src/main.c
git add src/main.c src/main_copy.c
c -m "Modify main.c and copy it"

# copy of an unmodified file: shown as A by libgit2's defaults
cp LICENSE docs/LICENSE
git add docs/LICENSE
c -m "Copy LICENSE into docs"

# typechange: file -> symlink
git rm -q docs/LICENSE
mkdir -p docs
ln -s ../LICENSE docs/LICENSE
git add docs/LICENSE
c -m "Turn docs/LICENSE into a symlink"

# plain symlink
ln -s README.md README.link
git add README.link
c -m "Add symlink"

# executable file
printf '#!/bin/sh\necho hi\n' > run.sh
chmod +x run.sh
git add run.sh
c -m "Add executable"

# submodule entry pointing at a commit that is not in this repo
cat > .gitmodules <<'EOF'
[submodule "vendor/lib"]
	path = vendor/lib
	url = https://example.com/lib.git
EOF
git add .gitmodules
git update-index --add --cacheinfo 160000,1234567890abcdef1234567890abcdef12345678,vendor/lib
c -m "Add submodule"

# odd paths: spaces, #, %, +, unicode, quotes
mkdir -p "odd dir"
printf 'spaces\n' > "odd dir/with space.txt"
printf 'hash\n' > "odd dir/a#b.txt"
printf 'percent\n' > "odd dir/100%.txt"
printf 'plus\n' > "odd dir/c+d.txt"
printf 'unicode\n' > "odd dir/ümlaut.txt"
printf 'quote\n' > "odd dir/it's.txt"
printf '<b>&amp;</b>\n' > "odd dir/x<y>&z.txt"
git add "odd dir"
c -m "Add files with odd names"

# non-UTF-8 path and content (skipped if the filesystem refuses the name)
printf 'caf\xe9 au lait\n\xff\xfe\n' > latin1.txt
git add latin1.txt
if printf '' > "$(printf 'caf\xe9.txt')" 2>/dev/null; then
	printf 'latin1 name\n' > "$(printf 'caf\xe9.txt')"
	git add "$(printf 'caf\xe9.txt')"
fi
c -m "Add non-UTF-8 bytes"

# a tag on an old commit, annotated and lightweight
T=$((T + 1)); GIT_COMMITTER_DATE="@$T +0100" git tag -a v0.1 -m "Version 0.1" HEAD~10
git tag v0.1-light HEAD~9
# a tag pointing at a tree (stagit stops iterating refs there; streagit skips it).
# Named to sort last so that stagit still lists the real tags.
git tag zz-tree-tag "$(git rev-parse HEAD^{tree})"

# branch with a few commits, then merge
git checkout -q -b feature
printf 'feature work\n' > feature.txt
git add feature.txt
c -m "Feature: step 1"
printf 'more feature work\n' >> feature.txt
git add feature.txt
c -m "Feature: step 2"
git checkout -q main
printf 'main work\n' > mainline.txt
git add mainline.txt
c -m "Main: parallel change"
T=$((T + 3600)); GIT_AUTHOR_DATE="@$T +0100" GIT_COMMITTER_DATE="@$T +0100" git merge -q --no-ff -m "Merge branch 'feature'" feature

# commit with an empty message, author != committer, negative offset
T=$((T + 3600))
printf 'x\n' > empty-msg.txt
git add empty-msg.txt
GIT_AUTHOR_NAME="Bob O'Brien" GIT_AUTHOR_EMAIL="bob@example.com" GIT_AUTHOR_DATE="@$T -0500" GIT_COMMITTER_DATE="@$T +0100" git commit -q --allow-empty-message -m ""

# a few more commits so paging has something to do (page size 10 in tests)
i=1; while [ $i -le 12 ]; do
	echo "change $i" >> changes.txt
	git add changes.txt
	c -m "Change $i"
	i=$((i + 1))
done

# multi-hunk change in big.txt with function-like context
sed -i.bak -e 's/^line 100$/def line_100():/' -e 's/^line 105$/LINE 105/' -e 's/^line 5000$/LINE 5000/' -e '$ d' big.txt && rm big.txt.bak
git add big.txt
c -m "Edit big.txt in several places"

# annotated tag on HEAD, with a message
T=$((T + 1)); GIT_COMMITTER_DATE="@$T +0100" git tag -a v1.0 -m "Version 1.0"
git branch stable HEAD~3

# ---------------------------------------------------------------- bare mirrors
SLOT="$ROOT/github.com/alice/repos/fixture"
mkdir -p "$SLOT"
git clone -q --bare --no-hardlinks "$WORK/main" "$SLOT/repo.git"
cd "$SLOT/repo.git"
git symbolic-ref HEAD refs/heads/main
# preserved refs that must be ignored, pointing at an old state
git update-ref refs/gitbackup/versions/20240101T000000/heads/main "$(git rev-parse 'main~20')"
git update-ref refs/gitbackup/versions/20240101T000000/tags/old-tag "$(git rev-parse 'main~21')"
git pack-refs --all
meta "$SLOT/repo.git" "Fixture repository & more" "https://github.com/alice/fixture" "alice"
rm -f "$SLOT/repo.git/hooks/"*.sample

# frozen version with relative alternates: only refs, objects borrowed
mkdir -p "$SLOT/repo.v1.git"
cd "$SLOT/repo.v1.git"
git init -q --bare -b main
printf '../../repo.git/objects\n' > objects/info/alternates
git update-ref refs/heads/main "$(git -C "$SLOT/repo.git" rev-parse 'main~15')"
git update-ref refs/tags/v0.1 "$(git -C "$SLOT/repo.git" rev-parse 'v0.1')"
git symbolic-ref HEAD refs/heads/main
git pack-refs --all
meta "$SLOT/repo.v1.git" "Fixture repository (version 1)" "https://github.com/alice/fixture" "alice"

# wiki
git_init "$WORK/wiki"
cd "$WORK/wiki"
printf '# Wiki\n' > Home.md
git add Home.md
c -m "Create wiki"
git clone -q --bare --no-hardlinks "$WORK/wiki" "$SLOT/wiki.git"
git -C "$SLOT/wiki.git" symbolic-ref HEAD refs/heads/main
git -C "$SLOT/wiki.git" pack-refs --all
meta "$SLOT/wiki.git" "Fixture wiki" "https://github.com/alice/fixture.wiki" "alice"

# fork: own commit on top of the parent's history, objects via alternates
mkdir -p "$SLOT/forks/bob/repo.git"
cd "$SLOT/forks/bob/repo.git"
git init -q --bare -b main
printf '../../../../repo.git/objects\n' > objects/info/alternates
git fetch -q "$SLOT/repo.git" main:refs/heads/main
cd "$WORK"
git clone -q --shared "$SLOT/forks/bob/repo.git" forkwork
cd forkwork
printf 'bob was here\n' > BOB.txt
git add BOB.txt
GIT_AUTHOR_NAME="Bob" GIT_AUTHOR_EMAIL="bob@example.com" c -m "Bob's fork commit"
git push -q origin main
cd "$SLOT/forks/bob/repo.git"
git symbolic-ref HEAD refs/heads/main
git pack-refs --all
meta "$SLOT/forks/bob/repo.git" "Bob's fork" "https://github.com/bob/fixture" "bob"

# fork version: nested alternates (-> fork -> parent)
mkdir -p "$SLOT/forks/bob/repo.v1.git"
cd "$SLOT/forks/bob/repo.v1.git"
git init -q --bare -b main
printf '../../repo.git/objects\n' > objects/info/alternates
git update-ref refs/heads/main "$(git -C "$SLOT/forks/bob/repo.git" rev-parse main)"
git symbolic-ref HEAD refs/heads/main
git pack-refs --all
meta "$SLOT/forks/bob/repo.v1.git" "Bob's fork (version 1)" "https://github.com/bob/fixture" "bob"

# a repo literally named "html"
git_init "$WORK/htmlrepo"
cd "$WORK/htmlrepo"
printf 'html repo\n' > index.txt
git add index.txt
c -m "html repo commit"
mkdir -p "$ROOT/github.com/alice/repos/html"
git clone -q --bare --no-hardlinks "$WORK/htmlrepo" "$ROOT/github.com/alice/repos/html/repo.git"
git -C "$ROOT/github.com/alice/repos/html/repo.git" symbolic-ref HEAD refs/heads/main
meta "$ROOT/github.com/alice/repos/html/repo.git" "A repo named html" "https://github.com/alice/html" "alice"

# empty repo
mkdir -p "$ROOT/github.com/alice/repos/empty"
git init -q --bare -b main "$ROOT/github.com/alice/repos/empty/repo.git"
meta "$ROOT/github.com/alice/repos/empty/repo.git" "Empty repository" "" "alice"

# gist
git_init "$WORK/gist"
cd "$WORK/gist"
printf 'print("gist")\n' > snippet.py
git add snippet.py
c -m "gist"
mkdir -p "$ROOT/github.com/alice/gists/abc123"
git clone -q --bare --no-hardlinks "$WORK/gist" "$ROOT/github.com/alice/gists/abc123/repo.git"
git -C "$ROOT/github.com/alice/gists/abc123/repo.git" symbolic-ref HEAD refs/heads/main
meta "$ROOT/github.com/alice/gists/abc123/repo.git" "A gist" "https://gist.github.com/abc123" "alice"

# paging: a history where a date-ordered walk pops a branch whose exclusive
# ancestors are older than the other branch's commits. Walk from main:
#   M2 M1 x2 y2 y1 x1 base
# A page ending at x2 whose next page restarted the walk at y2 alone would
# never show x1 (only reachable through x2).
git_init "$WORK/paging"
cd "$WORK/paging"
printf 'base\n' > f
git add f
c -m "base"
git checkout -q -b x
printf 'x1\n' >> f
git add f
c -m "x1"
git checkout -q main
git checkout -q -b y
printf 'y1\n' > g
git add g
c -m "y1"
printf 'y2\n' >> g
git add g
c -m "y2"
git checkout -q x
printf 'x2\n' >> f
git add f
c -m "x2"
git checkout -q main
T=$((T + 3600)); GIT_AUTHOR_DATE="@$T +0100" GIT_COMMITTER_DATE="@$T +0100" git merge -q --no-ff -m "merge x" x
T=$((T + 3600)); GIT_AUTHOR_DATE="@$T +0100" GIT_COMMITTER_DATE="@$T +0100" git merge -q --no-ff -m "merge y" y
mkdir -p "$ROOT/github.com/alice/repos/paging"
git clone -q --bare --no-hardlinks "$WORK/paging" "$ROOT/github.com/alice/repos/paging/repo.git"
git -C "$ROOT/github.com/alice/repos/paging/repo.git" symbolic-ref HEAD refs/heads/main
meta "$ROOT/github.com/alice/repos/paging/repo.git" "Paging" "https://github.com/alice/paging" "alice"

# star view symlink (inside the root) and an escaping symlink
mkdir -p "$ROOT/github.com/carol/stars/alice"
ln -sfn ../../../alice/repos/fixture "$ROOT/github.com/carol/stars/alice/fixture"
OUTSIDE="$ROOT/../outside-$(basename "$ROOT")"
mkdir -p "$OUTSIDE"
git init -q --bare -b main "$OUTSIDE/repo.git"
mkdir -p "$ROOT/github.com/alice/repos/evil"
ln -sfn "$OUTSIDE/repo.git" "$ROOT/github.com/alice/repos/evil/repo.git"

echo "fixture built in $ROOT"
