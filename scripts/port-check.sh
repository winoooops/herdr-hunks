#!/bin/sh
# Verifies src/git/ == pinned vimeflow sources + port/patches/*.patch, byte for byte.
set -eu
PIN=91e45b1c
src="${1:?usage: port-check.sh <vimeflow-checkout>}"
invalid_names="$(find port/patches -name '*[[:space:]]*' -print)"
if [ -n "$invalid_names" ]; then
  printf 'port-check: patch names must not contain whitespace:\n%s\n' "$invalid_names" >&2
  exit 1
fi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$src" cat-file -e "$PIN^{commit}" || { echo "pin $PIN not found in $src" >&2; exit 2; }
mkdir -p "$work/src/git"
for f in mod.rs watcher.rs test_helpers.rs; do
  git -C "$src" show "$PIN:crates/backend/src/git/$f" > "$work/src/git/$f"
done
for p in port/patches/*.patch; do
  [ -e "$p" ] || continue
  # The registry parser below only sees canonical names, so nothing else may exist.
  case "${p##*/}" in
    *[!A-Za-z0-9._-]*) canonical=no ;;
    [0-9][0-9][0-9][0-9]-*.patch) canonical=yes ;;
    *) canonical=no ;;
  esac
  if [ "$canonical" = no ]; then
    echo "port-check: patch ${p##*/} must be named NNNN-<name>.patch using only letters, digits, '.', '_' and '-'" >&2
    exit 1
  fi
  if ! grep -Fq -e "${p##*/}" PORT-SURFACE.md; then
    echo "port-check: patch ${p##*/} is not registered in PORT-SURFACE.md" >&2
    exit 1
  fi
  patch -s -d "$work" -p1 < "$p"
done
awk '{
  while (match($0, /[0-9][0-9][0-9][0-9]-[^[:space:]`\/]*[.]patch/)) {
    print substr($0, RSTART, RLENGTH)
    $0 = substr($0, RSTART + RLENGTH)
  }
}' PORT-SURFACE.md > "$work/registered-patches"
while IFS= read -r p; do
  if [ ! -f "port/patches/$p" ]; then
    echo "port-check: registered patch $p is missing from port/patches/" >&2
    exit 1
  fi
done < "$work/registered-patches"
non_regular="$(find src/git ! -type f ! -type d -print)"
if [ -n "$non_regular" ]; then
  printf 'port-check: src/git allows only regular files and directories; invalid entries:\n%s\n' "$non_regular" >&2
  exit 1
fi
diff -ru "$work/src/git" src/git
echo "port-check: src/git matches $PIN + $(ls port/patches/*.patch 2>/dev/null | wc -l | tr -d ' ') patch(es)"
