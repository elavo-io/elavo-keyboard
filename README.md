# elavo-keyboard

An iPadOS-style on-screen keyboard for Elavo on touchscreens. The daemon is
also the session's input method: it adds a period on double-space for
physical keys too, which Folio's `elavo-input` did before. The Elavo image
starts `elavo-keyboard` in place of `elavo-input`.

## What is here

- `crates/elavo-keyboard-widget`: the keyboard as a plain GTK4 widget. It
  draws the keys and reports taps as actions (`Text`, `Backspace`, `Return`,
  `Hide`). It knows nothing about Wayland, so the greeter can later put the
  same widget in its own window and type into its own `gtk::Entry`.
  - iPadOS landscape layout: letters, `.?123` numbers and `#+=` symbols.
  - Shift: one tap for the next letter, a quick double tap for caps lock.
  - Delete repeats while held (after 450 ms, then every 70 ms).
  - Dark mode first. Shift, delete and hide icons are drawn with cairo, so
    they don't depend on installed fonts or icon themes.
  - Sizes: Compact, Standard and Large, as a share of the screen height
    with a floor and a ceiling. Standard is 292 px on a 768 px screen: a
    36 px suggestion strip (empty for now), 4 rows of 54 px keys and 8 px
    gaps.
- `crates/elavo-keyboard`: the `elavo-keyboard` daemon for labwc.
  - Shows the widget in a gtk4-layer-shell surface on the overlay layer,
    anchored to the bottom, with an exclusive zone, so maximized windows
    shrink to fit above it. It never takes keyboard focus.
  - Connects as the seat's input method (input-method-unstable-v2). Text is
    committed with `commit_string`, and delete uses
    `delete_surrounding_text` when the field reports its text. Return, and
    delete when there is a selection or no surrounding text, go through a
    virtual keyboard (virtual-keyboard-unstable-v1).
  - Shows on `activate` (a text field took focus), as the setting below
    allows, and hides on `deactivate`, with a 120 ms delay so moving
    between fields doesn't flicker.
  - Holds the keyboard grab and passes every physical key back through the
    virtual keyboard, except the second space of a quick pair (it becomes
    `. `) and the Backspace that undoes it. The rule, ported unchanged from
    `elavo-input`, is in `src/period/decide.rs`: within 1.1 s, after a
    letter or digit, not in password, PIN or terminal fields, and not with
    a modifier held. On-screen spaces follow the same rule. It is on unless
    `~/.config/elavo/keyboard.conf` has `[Text]
    AddPeriodWithDoubleSpace=false` (the Keyboard pane in System Settings).
  - The Wayland side runs on its own thread and connection, and talks to
    GTK through channels.
  - `examples/test-entry.rs`: a dark test window with an entry, a
    multi-line field and a button. It prints what the fields contain.
  - `examples/tap-pointer.rs`: presses a screen point through
    wlr-virtual-pointer, so the test scripts can tap and hold keys.
  - `examples/press-keys.rs`: presses keys through a separate virtual
    keyboard with real key codes, standing in for a physical keyboard.

labwc lets only one input method connect per seat. If another one, such as
`elavo-input`, already has it, `elavo-keyboard` says so and exits. The
systemd unit conflicts with `elavo-input.service` so only one starts.

## When it shows

The Touchscreen pane in System Settings writes
`~/.config/elavo/onscreen-keyboard.conf`. The daemon watches the file and
applies changes at once.

```ini
[OnScreenKeyboard]
Mode=automatic   # automatic, always or never
Size=standard    # compact, standard or large
```

- **Automatic** (the default): shows when a text field takes focus within
  1.5 s of a touchscreen or stylus contact, unless a physical key was
  pressed since. A physical key hides it. With no physical keyboard attached
  at all (a tablet, or a detached keyboard), it shows for every text field.
- **Always**: shows for every text field.
- **Never**: never shows. The double-space period still works.

`ELAVO_KEYBOARD_MODE` and `ELAVO_KEYBOARD_SIZE` override the file.

Wayland doesn't tell one client about another's input, and `/dev/input`
needs root, so Automatic reads two files anyone can read:
`/proc/bus/input/devices` for keyboards (letters, space and Enter) and
touchscreens, and `/proc/interrupts` for the touchscreen's interrupt count,
sampled every 200 ms. A rising count is a touch. This works for I2C
touchscreens (the usual laptop and tablet kind, such as `ELAN2097:00`). A
USB touchscreen shares its interrupt with other USB devices, so with one of
those Automatic only goes by whether a keyboard is attached.

## Install

```bash
cargo build --release --locked
DESTDIR=/out scripts/install   # bin, systemd user unit, license; PREFIX defaults to /usr
```

The image enables the unit with `systemctl --global enable
elavo-keyboard.service`.

## Build

```bash
toolbox run -c elavo-dev cargo build
toolbox run -c elavo-dev cargo test
```

## Try it in a nested session

`scripts/nested-session.sh` starts a headless labwc on its own socket
(usually `wayland-1`) at 1366x768, with an empty autostart, so nothing from
the real session runs there and the real session is left alone. Programs run
inside the `elavo-dev` toolbox. Nothing needs root.

```bash
scripts/nested-session.sh start        # headless labwc, 1366x768
scripts/nested-session.sh keyboard     # elavo-keyboard, logging to /tmp/elavo-keyboard-test/keyboard.log
scripts/nested-session.sh entry        # the test window; its entry has focus, so the keyboard shows
scripts/nested-session.sh tap shift h i space t h e r e
scripts/nested-session.sh hold backspace 800
scripts/nested-session.sh tap return   # the entry prints "entry activated"
scripts/nested-session.sh press h i space space   # physical keys: "hi. "
scripts/nested-session.sh mode never   # the daemon applies it at once
scripts/nested-session.sh shot keyboard  # ~/GitHub/.lens-preview/keyboard.png
scripts/nested-session.sh stop
```

Key ids are the characters themselves plus `shift`, `space`, `backspace`,
`return`, `hide`, `page-letters`, `page-numbers` and `page-symbols`. The
daemon writes every key's screen center to `/tmp/elavo-keyboard-test/keys`
each time it shows or changes page. In the nested session the daemon reads
its settings from `/tmp/elavo-keyboard-test/config`, not yours, and a fake
touchscreen interrupt count that every `click` and `tap` raises, so
Automatic can be tried there. `NO_KEYBOARD=1 scripts/nested-session.sh
keyboard` makes it see no physical keyboard. `taps` sends several taps in one go, fast
enough for a double tap on shift. `MODE=1920x1080` picks another screen size.

To watch it instead of taking screenshots, run labwc nested in a window:
start `labwc -C /tmp/elavo-keyboard-test/labwc` from a terminal in your
session, then run the daemon and test window with `WAYLAND_DISPLAY` set to
its socket.

## Not yet

- A manual open button.
- Reopening after the hide key: tapping the same field again doesn't send a
  new `activate`, so the keyboard stays hidden until focus leaves and comes
  back.
- The greeter and lock screen (in-process widget, login box moves up).
- Suggestions, autocorrect, more layouts, a light theme, key sounds.
- Sliding floating windows above the keyboard (a labwc patch).

## License

MIT. See `LICENSE.md`.
