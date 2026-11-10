#!/bin/bash
# Run the keyboard in a separate, headless labwc so the real session (and
# its elavo-input) is left alone.
#
#   scripts/nested-session.sh start     labwc on a new socket, 1366x768
#   scripts/nested-session.sh keyboard  start elavo-keyboard in it
#   scripts/nested-session.sh entry     start the test window in it
#   scripts/nested-session.sh shot NAME screenshot to $SHOTS/NAME.png
#   scripts/nested-session.sh tap ID... tap keys by id (q, shift, space, backspace, return, hide, page-numbers)
#   scripts/nested-session.sh click X Y [HOLD_MS]  press a screen point
#   scripts/nested-session.sh hold ID MS hold one key down for MS milliseconds
#   scripts/nested-session.sh stop      end it all
#
# Programs run inside the elavo-dev toolbox, where they were built.
# Nothing here needs root.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
T=${ELAVO_KEYBOARD_TEST:-/tmp/elavo-keyboard-test}
SHOTS=${SHOTS:-$HOME/GitHub/.lens-preview}
MODE=${MODE:-1366x768}
BOX=${TOOLBOX:-elavo-dev}
mkdir -p "$T"

socket() {
    [ -s "$T/socket" ] || { echo "no nested session; run: $0 start" >&2; exit 1; }
    cat "$T/socket"
}

in_box() {
    toolbox run -c "$BOX" env -u GTK_THEME -u GSETTINGS_SCHEMA_DIR \
        WAYLAND_DISPLAY="$(socket)" GSK_RENDERER="${GSK_RENDERER:-cairo}" "$@"
}

case "${1:-}" in
start)
    mkdir -p "$T/labwc"
    # An empty autostart: nothing from the real Elavo session starts here,
    # in particular not elavo-input, so the keyboard can be the input method.
    : >"$T/labwc/autostart"
    cat >"$T/labwc/rc.xml" <<'XML'
<?xml version="1.0"?>
<labwc_config>
  <core><decoration>server</decoration></core>
  <placement><policy>center</policy></placement>
</labwc_config>
XML
    rm -f "$T/socket"
    env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless WLR_RENDERER=pixman \
        WLR_HEADLESS_OUTPUTS=1 XDG_CONFIG_HOME="$T/config" \
        setsid labwc -C "$T/labwc" -s "sh -c 'echo \$WAYLAND_DISPLAY >$T/socket'" \
        >"$T/labwc.log" 2>&1 &
    echo $! >"$T/labwc.pid"
    for _ in $(seq 50); do [ -s "$T/socket" ] && break; sleep 0.1; done
    S=$(socket)
    WAYLAND_DISPLAY=$S wlr-randr --output HEADLESS-1 --custom-mode "$MODE" >/dev/null
    echo "nested labwc on $S at $MODE (pid $(cat "$T/labwc.pid"))"
    ;;
keyboard)
    in_box env ELAVO_KEYBOARD_LOG=1 ELAVO_KEYBOARD_KEYS_FILE="$T/keys" \
        "$ROOT/target/debug/elavo-keyboard" >"$T/keyboard.log" 2>&1 &
    sleep 1.5
    cat "$T/keyboard.log"
    ;;
entry)
    in_box env TEST_MAXIMIZE=1 "$ROOT/target/debug/examples/test-entry" >"$T/entry.log" 2>&1 &
    sleep 2
    ;;
shot)
    mkdir -p "$SHOTS"
    out="$SHOTS/${2:-keyboard}.png"
    WAYLAND_DISPLAY=$(socket) grim -o HEADLESS-1 "$out"
    echo "$out"
    ;;
click)
    in_box "$ROOT/target/debug/examples/tap-pointer" "$2" "$3" "${4:-40}" ${MODE/x/ }
    ;;
hold)
    # hold ID MS: press one key for MS milliseconds (for key repeat).
    line=$(awk -v id="$2" '$1 == id {print; exit}' "$T/keys")
    [ -n "$line" ] || { echo "no key $2" >&2; exit 1; }
    read -r _ x y <<<"$line"
    "$0" click "$x" "$y" "$3"
    ;;
tap)
    # Each id is looked up when its turn comes, since page keys change the
    # layout; the pause lets a page switch rewrite the key list first.
    shift
    for id in "$@"; do
        sleep 0.25
        line=$(awk -v id="$id" '$1 == id {print; exit}' "$T/keys")
        [ -n "$line" ] || { echo "no key $id" >&2; exit 1; }
        read -r _ x y <<<"$line"
        "$0" click "$x" "$y"
    done
    ;;
taps)
    # Several taps inside one toolbox call, fast enough for a double tap.
    # Only for keys on the current page.
    shift
    cmd=""
    for id in "$@"; do
        line=$(awk -v id="$id" '$1 == id {print; exit}' "$T/keys")
        [ -n "$line" ] || { echo "no key $id" >&2; exit 1; }
        read -r _ x y <<<"$line"
        cmd="$cmd $ROOT/target/debug/examples/tap-pointer $x $y 30 ${MODE/x/ };"
    done
    in_box sh -c "$cmd"
    ;;
stop)
    pkill -f "$ROOT/target/debug/elavo-keyboard" || true
    pkill -f "$ROOT/target/debug/examples/test-entry" || true
    [ -s "$T/labwc.pid" ] && kill "$(cat "$T/labwc.pid")" 2>/dev/null || true
    rm -f "$T/socket" "$T/labwc.pid"
    ;;
*)
    sed -n '2,15p' "$0"
    exit 1
    ;;
esac
