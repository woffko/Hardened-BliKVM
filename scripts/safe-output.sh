#!/bin/sh
# Sourced by the third-party build scripts before they replace their output
# directory. `rm -rf` runs only on a directory an earlier run of the same
# script made (it holds the SOURCE file) or on an empty or absent one, and
# never on the filesystem root, the home directory or a path too shallow to
# be a build directory.
refuse_unsafe_output() {
  candidate=$1
  case "$candidate" in
    /*) ;;
    *) echo "output directory must be absolute" >&2; return 2 ;;
  esac
  trimmed=${candidate%/}
  case "$trimmed" in
    ""|"${HOME:-/nonexistent}") echo "refusing to replace $candidate" >&2; return 2 ;;
  esac
  depth=$(printf '%s' "$trimmed" | tr -cd / | wc -c)
  if [ "$depth" -lt 3 ]; then
    echo "refusing to replace $candidate: too close to the root" >&2
    return 2
  fi
  if [ -e "$candidate" ]; then
    if [ ! -d "$candidate" ] || [ -L "$candidate" ]; then
      echo "refusing to replace $candidate: not a plain directory" >&2
      return 2
    fi
    if [ ! -f "$candidate/SOURCE" ] && [ -n "$(ls -A "$candidate" 2>/dev/null)" ]; then
      echo "refusing to replace $candidate: it was not made by this script" >&2
      return 2
    fi
  fi
  return 0
}
