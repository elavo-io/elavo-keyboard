//! Press the pointer at a screen point through wlr-virtual-pointer, for the
//! nested test session. Holding lets the test check key repeat.
//!
//! tap-pointer X Y [HOLD_MS] [WIDTH HEIGHT]

use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const BTN_LEFT: u32 = 0x110;

struct State;

fn main() {
    let args: Vec<u32> = std::env::args()
        .skip(1)
        .map(|a| a.parse::<f64>().expect("numbers only").round() as u32)
        .collect();
    let (x, y) = (args[0], args[1]);
    let hold = args.get(2).copied().unwrap_or(40);
    let (w, h) = (
        args.get(3).copied().unwrap_or(1366),
        args.get(4).copied().unwrap_or(768),
    );

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=7, ()).expect("seat");
    let manager: ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .expect("no wlr-virtual-pointer");
    let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
    let start = Instant::now();
    let t = || start.elapsed().as_millis() as u32;

    pointer.motion_absolute(t(), x, y, w, h);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    pointer.button(t(), BTN_LEFT, wl_pointer::ButtonState::Pressed);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    std::thread::sleep(Duration::from_millis(u64::from(hold)));
    pointer.button(t(), BTN_LEFT, wl_pointer::ButtonState::Released);
    pointer.frame();
    queue.roundtrip(&mut State).unwrap();
    pointer.destroy();
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
impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
