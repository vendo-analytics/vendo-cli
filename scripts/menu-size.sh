#!/usr/bin/env bash
# What the arrow-key menu of a group run without its command adds to the release binary
# (VE-3826). Yalcin accepted its size on 2026-10-06 and then said going over 150 KB is fine, so this
# measures and reports, and only warns when the menu adds 150,000 bytes or more; it never fails.
# It builds the release binary without the menu (the `menu` cargo feature: inquire, crossterm's key
# events and the menu's code) and with it. release.yml runs it for each target and the sizes go
# to the run's summary.
#
#   scripts/menu-size.sh                    # this machine's target
#   scripts/menu-size.sh --target <triple>  # another target, as release.yml builds it
#
# The binary with the menu is built last, so the release binary it leaves in target/ is the
# one releases ship. Works with macOS's bash 3.2.
set -euo pipefail

# A warning, not a limit (Yalcin, 2026-10-06): 150 KB counted in thousands, as the decision sheet does.
LIMIT=150000
KB=$((LIMIT / 1000))

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

cd "$(dirname "$0")/.."

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
if [ "$added" -ge "$LIMIT" ]; then
  verdict="The menu adds $KB KB or more ($(grouped "$LIMIT") bytes): reported, not a failure."
else
  verdict="The menu adds under $KB KB ($(grouped "$LIMIT") bytes)."
fi

echo
echo "vendo release binary, $target:"
printf '  with the menu     %12s bytes\n' "$(grouped "$with")"
printf '  without the menu  %12s bytes\n' "$(grouped "$without")"
printf '  the menu adds     %12s bytes (%s), warning at: %s bytes (%s KB)\n' \
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
| Warning at | $(grouped "$LIMIT") ($KB KB) |

$verdict
EOF
fi

if [ "$added" -ge "$LIMIT" ] && [ "${GITHUB_ACTIONS:-}" = true ]; then
  echo "::warning::The menu adds $(grouped "$added") bytes to the $target binary, $KB KB ($(grouped "$LIMIT") bytes) or more. Reported only (VE-3826)."
fi
