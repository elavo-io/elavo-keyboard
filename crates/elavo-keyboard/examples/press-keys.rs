//! Press keys like a physical US keyboard would, for the nested test
//! session. Unlike `wtype`, it sends real evdev key codes with a standard
//! keymap, so the input method sees the same codes a laptop keyboard sends.
//!
//! press-keys [--gap MS] KEY...
//!
//! A KEY is a lowercase letter, a digit, `space`, `backspace`, `enter`, `tab`,
//! `period` or `comma`. The gap between presses defaults to 120 ms.

use std::io::Write;
use std::os::fd::{AsFd, FromRawFd};
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

const KEYMAP: &str = "xkb_keymap {\n\
    xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n\
    xkb_types { include \"complete\" };\n\
    xkb_compat { include \"complete\" };\n\
    xkb_symbols { include \"pc+us+inet(evdev)\" };\n\
};\n";

fn code(name: &str) -> u32 {
    const ROWS: [(&str, u32); 4] = [
        ("1234567890", 2),
        ("qwertyuiop", 16),
        ("asdfghjkl", 30),
        ("zxcvbnm", 44),
    ];
    match name {
        "space" => return 57,
        "backspace" => return 14,
        "enter" => return 28,
        "tab" => return 15,
        "period" => return 52,
        "comma" => return 51,
        _ => {}
    }
    for (row, first) in ROWS {
        if let Some(i) = row.find(name).filter(|_| name.len() == 1) {
            return first + i as u32;
        }
    }
    panic!("unknown key {name}");
}

struct State;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut gap = 120;
    if args.first().is_some_and(|a| a == "--gap") {
        gap = args[1].parse().expect("gap in ms");
        args.drain(..2);
    }

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=7, ()).expect("seat");
    let manager: ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .expect("no virtual-keyboard-v1");
    let vk = manager.create_virtual_keyboard(&seat, &qh, ());

    let fd = unsafe { libc::memfd_create(c"press-keys".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create");
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut bytes = KEYMAP.as_bytes().to_vec();
    bytes.push(0);
    file.write_all(&bytes).unwrap();
    vk.keymap(1, file.as_fd(), bytes.len() as u32);
    queue.roundtrip(&mut State).unwrap();

    let start = Instant::now();
    let t = || start.elapsed().as_millis() as u32;
    for (i, name) in args.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(gap));
        }
        let key = code(name);
        vk.key(t(), key, 1);
        queue.roundtrip(&mut State).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        vk.key(t(), key, 0);
        queue.roundtrip(&mut State).unwrap();
    }
    std::thread::sleep(Duration::from_millis(50));
    vk.destroy();
    queue.roundtrip(&mut State).unwrap();
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
