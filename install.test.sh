#!/usr/bin/env bash
# Drives the arrow-key helpers with synthetic keystrokes.
set -uo pipefail

SRC=${1:-/var/www/Quanta-Rust/install.sh}
FN=$(mktemp /tmp/nav.XXXXXX.sh)
awk '/^# ─── Arrow-key navigation/,/^# ─── System/' "$SRC" | head -n -1 > "$FN"
[ -s "$FN" ] || { echo "could not extract helpers from $SRC"; exit 1; }

RED=; GREEN=; YELLOW=; BLUE=; CYAN=; BOLD=; DIM=; NC=
banner() { :; }; print_banner() { :; }; sep() { :; }; sep_bold() { :; }
. "$FN"

KEYS=$(mktemp /tmp/keys.XXXXXX)
fails=0
check() {
    if [ "$2" = "$3" ]; then echo "  ok   $1"
    else echo "  FAIL $1 - expected '$2', got '$3'"; fails=$((fails+1)); fi
}
run_menu() {
    local rc
    MENU_CHOICE=x
    printf '%b' "$1" > "$KEYS"
    menu "T" "A" "B" "C" >/dev/null 2>&1 < "$KEYS"
    rc=$?
    echo "${rc}:${MENU_CHOICE}"
}
run_confirm() {
    printf '%b' "$1" > "$KEYS"
    confirm "Q?" "$2" >/dev/null 2>&1 < "$KEYS"
    echo $?
}

echo "menu:"
check "Enter selects first item"       "0:0" "$(run_menu '\n')"
check "Down then Enter selects second" "0:1" "$(run_menu '\033[B\n')"
check "Up wraps to last item"          "0:2" "$(run_menu '\033[A\n')"
check "Down x4 wraps to second"        "0:1" "$(run_menu '\033[B\033[B\033[B\033[B\n')"
check "vim keys j/k move"              "0:1" "$(run_menu 'jkj\n')"
check "q backs out"                    "1:x" "$(run_menu 'q')"

echo "confirm:"
check "Enter takes y default"          "0" "$(run_confirm '\n' y)"
check "Enter takes n default"          "1" "$(run_confirm '\n' n)"
check "arrow toggles y default to No"  "1" "$(run_confirm '\033[C\n' y)"
check "arrow toggles n default to Yes" "0" "$(run_confirm '\033[D\n' n)"
check "n key selects No"               "1" "$(run_confirm 'n\n' y)"
check "y key selects Yes"              "0" "$(run_confirm 'y\n' n)"

rm -f "$FN" "$KEYS"
[ "$fails" -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
