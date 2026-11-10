# elavo-keyboard

An experiment: an iPadOS-style on-screen keyboard for Elavo on touchscreens.
It lives on its own while the design settles, and does not change any other
Elavo repository.

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
  - Shows on `activate` (a text field took focus) and hides on `deactivate`,
    with a 120 ms delay so moving between fields doesn't flicker.
  - The Wayland side runs on its own thread and connection, and talks to
    GTK through channels.
  - `examples/test-entry.rs`: a dark test window with an entry, a
    multi-line field and a button. It prints what the fields contain.
  - `examples/tap-pointer.rs`: presses a screen point through
    wlr-virtual-pointer, so the test scripts can tap and hold keys.

labwc lets only one input method connect per seat. In a normal Elavo session
`elavo-input` (double-space period) already has it, so `elavo-keyboard` says
so and exits. Try it in the nested session below instead.

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
scripts/nested-session.sh shot keyboard  # ~/GitHub/.lens-preview/keyboard.png
scripts/nested-session.sh stop
```

Key ids are the characters themselves plus `shift`, `space`, `backspace`,
`return`, `hide`, `page-letters`, `page-numbers` and `page-symbols`. The
daemon writes every key's screen center to `/tmp/elavo-keyboard-test/keys`
each time it shows or changes page. `taps` sends several taps in one go, fast
enough for a double tap on shift. `MODE=1920x1080` picks another screen size.

To watch it instead of taking screenshots, run labwc nested in a window:
start `labwc -C /tmp/elavo-keyboard-test/labwc` from a terminal in your
session, then run the daemon and test window with `WAYLAND_DISPLAY` set to
its socket.

## Settings stub

`~/.config/elavo/onscreen-keyboard.conf`:

```ini
[OnScreenKeyboard]
Size=standard   # compact, standard or large
```

`ELAVO_KEYBOARD_SIZE` overrides it. The Touchscreen pane in System Settings
will own these later.

## Not yet

- Only showing when no physical keyboard is attached, and a manual open
  button.
- Reopening after the hide key: tapping the same field again doesn't send a
  new `activate`, so the keyboard stays hidden until focus leaves and comes
  back.
- Merging with `elavo-input`, including the double-space period.
- The greeter and lock screen (in-process widget, login box moves up).
- Suggestions, autocorrect, more layouts, a light theme, key sounds.
- Sliding floating windows above the keyboard (a labwc patch).

## License

MIT. See `LICENSE.md`.
