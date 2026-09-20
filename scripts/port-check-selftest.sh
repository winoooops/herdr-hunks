#!/bin/sh
set -eu
[ "$#" -eq 1 ] || { echo "usage: port-check-selftest.sh <vimeflow-checkout>" >&2; exit 2; }
src="$(CDPATH= cd "$1" && pwd)"
root="$(CDPATH= cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/scripts" "$work/port" "$work/src"
cp "$root/scripts/port-check.sh" "$work/scripts/"
cp -R "$root/port/patches" "$work/port/"
cp -R "$root/src/git" "$work/src/"
cp "$root/PORT-SURFACE.md" "$work/"
cd "$work"

check() {
  if sh scripts/port-check.sh "$src" > "$work/result.log" 2>&1; then
    actual=pass
  else
    actual=fail
  fi
  if [ "$actual" != "$2" ]; then
    printf 'FAIL: %s (expected %s, got %s)\n' "$1" "$2" "$actual" >&2
    cat "$work/result.log" >&2
    exit 1
  fi
  printf 'PASS: %s\n' "$1"
}

check baseline pass

mv src/git/mod.rs "$work/mod.rs"
ln -s "$work/mod.rs" src/git/mod.rs
check 'symlinked mod.rs' fail
rm src/git/mod.rs
mv "$work/mod.rs" src/git/mod.rs

: > src/git/extra.rs
check 'extra file' fail
rm src/git/extra.rs

cp src/git/mod.rs "$work/mod.rs"
printf '\n// hand edit\n' >> src/git/mod.rs
check 'hand edit' fail
mv "$work/mod.rs" src/git/mod.rs

mv port/patches/0002-drain-sync-output.patch 'port/patches/0002-drain sync-output.patch'
sed 's/0002-drain-sync-output.patch/0002-drain sync-output.patch/g' PORT-SURFACE.md > "$work/registry"
mv "$work/registry" PORT-SURFACE.md
check 'registered patch name with whitespace' fail
grep -q 'patch names must not contain whitespace' "$work/result.log"
mv 'port/patches/0002-drain sync-output.patch' port/patches/0002-drain-sync-output.patch
cp "$root/PORT-SURFACE.md" PORT-SURFACE.md

mv port/patches/0002-drain-sync-output.patch port/patches/002-drain-sync-output.patch
sed 's/0002-drain-sync-output.patch/002-drain-sync-output.patch/g' PORT-SURFACE.md > "$work/registry"
mv "$work/registry" PORT-SURFACE.md
check 'registered patch with a non-canonical name' fail
grep -q 'must be named NNNN-<name>.patch' "$work/result.log"
mv port/patches/002-drain-sync-output.patch port/patches/0002-drain-sync-output.patch
cp "$root/PORT-SURFACE.md" PORT-SURFACE.md

mv port/patches/0002-drain-sync-output.patch port/patches/9999-unregistered.patch
check 'unregistered patch file' fail
mv port/patches/9999-unregistered.patch port/patches/0002-drain-sync-output.patch

rm port/patches/0001-no-ext-diff.patch
for f in mod.rs watcher.rs test_helpers.rs; do
  git -C "$src" show "91e45b1c:crates/backend/src/git/$f" > "src/git/$f"
done
for p in port/patches/*.patch; do
  patch -s -p1 < "$p"
done
check 'missing registered patch file' fail
