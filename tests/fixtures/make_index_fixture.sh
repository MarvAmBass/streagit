#!/bin/sh
# Build the plain-layout fixture for the index tests and the stagit-index
# golden page. Usage: make_index_fixture.sh <root>
#
#   alpha                 bare repo without .git suffix (root level)
#   beta.git              bare repo (root level)
#   c++                   name that needs percent-encoding (root level)
#   team/tool.git         nested
#   team/app              nested, no .git suffix
#   team/empty.git        no commits
#   mirrors/m.git         excluded by the tests
#   deep/a/b/c.git        four levels deep (beyond the default --index-depth)
#   links/team            symlink -> ../team (listed once, under team/)
#   links/loop            symlink -> .. (a loop)
#
# Descriptions and owners are written without a trailing newline so that
# stagit-index's output (which keeps it) is byte-identical to streagit's.
# Author dates differ from committer dates: the index shows the author time.
set -eu
mkdir -p "$1"
ROOT=$(cd "$1" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

export GIT_AUTHOR_NAME="Alice Example" GIT_AUTHOR_EMAIL="alice@example.com"
export GIT_COMMITTER_NAME="Alice Example" GIT_COMMITTER_EMAIL="alice@example.com"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null

# repo <path> <author epoch> <description> <owner>
repo() {
	git init -q --bare -b main "$ROOT/$1"
	rm -rf "$WORK/w"
	git init -q -b main "$WORK/w"
	printf '%s\n' "$1" > "$WORK/w/README"
	git -C "$WORK/w" add README
	GIT_AUTHOR_DATE="@$2 +0200" GIT_COMMITTER_DATE="@$(($2 + 86400 * 400)) +0000" \
		git -C "$WORK/w" commit -q -m "Initial commit of $1"
	git -C "$WORK/w" push -q "$ROOT/$1" main
	printf '%s' "$3" > "$ROOT/$1/description"
	printf '%s' "$4" > "$ROOT/$1/owner"
}

repo alpha 1700000000 'The alpha <project> & "friends"' "Alice"
repo beta.git 1690000000 "It's beta" "Bob"
repo c++ 1710000000 "" ""
repo team/tool.git 1720000000 "A tool" "Team"
repo team/app 1705000000 "An app" "Team"
repo mirrors/m.git 1730000000 "A mirror" ""
repo deep/a/b/c.git 1740000000 "Deep" ""
git init -q --bare -b main "$ROOT/team/empty.git"
printf 'Nothing yet' > "$ROOT/team/empty.git/description"
mkdir -p "$ROOT/links"
ln -s ../team "$ROOT/links/team"
ln -s .. "$ROOT/links/loop"
