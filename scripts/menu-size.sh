#!/usr/bin/env bash
# What the arrow-key menu of a group run without its command adds to the release binary
# (VE-3826). Yalcin accepted its size on 2026-10-06 on condition that it stays under 150 KB at
# each release, so this builds the release binary without the menu (the `menu` cargo feature:
# inquire, crossterm's key events and the menu's code) and with it, and fails when the menu adds
# more than 153,600 bytes. release-rc.yml runs it for each target and the sizes go to the run's
# summary; run it before tagging too:
#
#   scripts/menu-size.sh                    # this machine's target
#   scripts/menu-size.sh --target <triple>  # another target, as release-rc.yml builds it
#
# The binary with the menu is built last, so the release binary it leaves in rust/target is the
# one releases ship. Works with macOS's bash 3.2.
set -euo pipefail

LIMIT=153600 # 150 KB
KB=$((LIMIT / 1024))

usage() {
  echo "Usage: scripts/menu-size.sh [--target <triple>]"
}

target=""
while [ $# -gt 0 ]; do
  case "$1" in
    --target)
      [ $# -ge 2 ] || { usage >&2; exit 2; }
      target="$2"
      shift 2
      ;;
    --target=*)
      target="${1#--target=}"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "menu-size.sh: unexpected argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cd "$(dirname "$0")/../rust"

build=(cargo build --release --locked)
binary="${CARGO_TARGET_DIR:-target}"
if [ -n "$target" ]; then
  build+=(--target "$target")
  binary="$binary/$target/release/vendo"
else
  target="$(rustc -vV | sed -n 's/^host: //p')"
  binary="$binary/release/vendo"
fi

bytes() {
  wc -c <"$1" | tr -d ' '
}

# 1234567 -> 1,234,567
grouped() {
  local n="$1" sign="" out=""
  case "$n" in -*) sign="-" n="${n#-}" ;; esac
  while [ ${#n} -gt 3 ]; do
    out=",${n: -3}$out"
    n="${n:0:${#n}-3}"
  done
  echo "$sign$n$out"
}

echo "Building vendo for $target without the menu (--no-default-features)" >&2
"${build[@]}" --no-default-features
without="$(bytes "$binary")"

echo "Building vendo for $target with the menu" >&2
"${build[@]}"
with="$(bytes "$binary")"

added=$((with - without))
percent="$(awk -v added="$added" -v without="$without" 'BEGIN { printf "%+.2f%%", added * 100 / without }')"
if [ "$added" -gt "$LIMIT" ]; then
  verdict="Over the limit: the menu adds more than $KB KB."
else
  verdict="Within the limit."
fi

echo
echo "vendo release binary, $target:"
printf '  with the menu     %12s bytes\n' "$(grouped "$with")"
printf '  without the menu  %12s bytes\n' "$(grouped "$without")"
printf '  the menu adds     %12s bytes (%s), limit %s bytes (%s KB)\n' \
  "$(grouped "$added")" "$percent" "$(grouped "$LIMIT")" "$KB"
echo "$verdict"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  cat >>"$GITHUB_STEP_SUMMARY" <<EOF
### Menu size, $target

| vendo release binary | Bytes |
| --- | ---: |
| With the menu | $(grouped "$with") |
| Without the menu | $(grouped "$without") |
| The menu adds | $(grouped "$added") ($percent) |
| Limit | $(grouped "$LIMIT") ($KB KB) |

$verdict
EOF
fi

if [ "$added" -gt "$LIMIT" ]; then
  if [ "${GITHUB_ACTIONS:-}" = true ]; then
    echo "::error::The menu adds $(grouped "$added") bytes to the $target binary, more than the $(grouped "$LIMIT") ($KB KB) accepted for VE-3826."
  fi
  exit 1
fi
