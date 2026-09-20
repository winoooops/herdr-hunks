#!/bin/sh
# Verifies src/git/ == pinned vimeflow sources + port/patches/*.patch, byte for byte.
set -eu
PIN=91e45b1c
src="${1:?usage: port-check.sh <vimeflow-checkout>}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$src" cat-file -e "$PIN^{commit}" || { echo "pin $PIN not found in $src" >&2; exit 2; }
mkdir -p "$work/src/git"
for f in mod.rs watcher.rs test_helpers.rs; do
  git -C "$src" show "$PIN:crates/backend/src/git/$f" > "$work/src/git/$f"
done
for p in port/patches/*.patch; do
  [ -e "$p" ] || continue
  patch -s -d "$work" -p1 < "$p"
done
diff -ru "$work/src/git" src/git
echo "port-check: src/git matches $PIN + $(ls port/patches/*.patch 2>/dev/null | wc -l | tr -d ' ') patch(es)"
