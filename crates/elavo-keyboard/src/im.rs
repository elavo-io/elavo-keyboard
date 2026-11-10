//! The Wayland side: one input-method-v2 object for text, and a
//! virtual-keyboard-v1 device for keys that are not text.
//!
//! This runs on its own thread and Wayland connection, next to GTK's.
//! [`spawn`] returns a [`Commands`] handle for the UI to send edits, and a
//! channel of [`UiEvent`]s for showing and hiding the keyboard.

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc;
use std::time::Instant;

use wayland_client::backend::WaylandError;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{
    event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};

use crate::protocol::input_method::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use crate::protocol::input_method::zwp_input_method_v2::{Event as ImEvent, ZwpInputMethodV2};
use crate::protocol::input_method::zwp_text_input_v3::ContentPurpose;
use crate::protocol::virtual_keyboard::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use crate::protocol::virtual_keyboard::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

/// Linux evdev key codes the keyboard sends through virtual-keyboard. The
/// arrows are for the cursor keys that come with a later layout.
#[allow(dead_code)]
pub mod keys {
    pub const BACKSPACE: u32 = 14;
    pub const ENTER: u32 = 28;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const UP: u32 = 103;
    pub const DOWN: u32 = 108;
}

/// The compositor compiles this with the system XKB data. It only has to
/// cover the keys above, but a full US map keeps keysyms sensible.
const KEYMAP: &str = "xkb_keymap {\n\
    xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n\
    xkb_types { include \"complete\" };\n\
    xkb_compat { include \"complete\" };\n\
    xkb_symbols { include \"pc+us+inet(evdev)\" };\n\
};\n";

/// What the UI should do after a dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// A text field took focus.
    Show { purpose: Purpose },
    /// No text field has focus.
    Hide,
    /// Another input method has the seat (in a real Elavo session that is
    /// `elavo-input`).
    Unavailable,
}

/// The subset of text-input content purposes the keyboard will care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Purpose {
    #[default]
    Normal,
    Password,
    Number,
    Email,
    Url,
    Terminal,
}

fn purpose_of(p: WEnum<ContentPurpose>) -> Purpose {
    match p {
        WEnum::Value(ContentPurpose::Password) | WEnum::Value(ContentPurpose::Pin) => {
            Purpose::Password
        }
        WEnum::Value(ContentPurpose::Digits)
        | WEnum::Value(ContentPurpose::Number)
        | WEnum::Value(ContentPurpose::Phone) => Purpose::Number,
        WEnum::Value(ContentPurpose::Email) => Purpose::Email,
        WEnum::Value(ContentPurpose::Url) => Purpose::Url,
        WEnum::Value(ContentPurpose::Terminal) => Purpose::Terminal,
        _ => Purpose::Normal,
    }
}

/// The focused field's text around the cursor, as last reported, with our
/// own edits applied until the next report. Offsets are bytes.
#[derive(Debug, Default, Clone)]
struct Field {
    known: bool,
    text: String,
    cursor: usize,
    anchor: usize,
}

impl Field {
    fn insert(&mut self, s: &str) {
        if !self.known {
            return;
        }
        let (a, b) = (self.cursor.min(self.anchor), self.cursor.max(self.anchor));
        if b > self.text.len() || !self.text.is_char_boundary(a) || !self.text.is_char_boundary(b) {
            self.known = false;
            return;
        }
        self.text.replace_range(a..b, s);
        self.cursor = a + s.len();
        self.anchor = self.cursor;
    }

    /// Bytes of the character before the cursor, when it can be deleted
    /// with delete_surrounding_text.
    fn char_before(&self) -> Option<usize> {
        if !self.known || self.cursor != self.anchor || self.cursor == 0 {
            return None;
        }
        self.text
            .get(..self.cursor)
            .and_then(|s| s.chars().next_back())
            .map(char::len_utf8)
    }
}

#[derive(Default)]
struct Pending {
    activate: bool,
    deactivate: bool,
    surrounding: Option<(String, u32, u32)>,
    purpose: Option<Purpose>,
}

pub struct State {
    log: bool,
    serial: u32,
    active: bool,
    purpose: Purpose,
    field: Field,
    pending: Pending,
    events: Vec<UiEvent>,
}

impl State {
    fn log(&self, msg: &str) {
        if self.log {
            eprintln!("elavo-keyboard: {msg}");
        }
    }

    fn apply_done(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        let p = std::mem::take(&mut self.pending);
        if p.activate {
            self.field = Field::default();
            self.purpose = Purpose::Normal;
        }
        if let Some((text, cursor, anchor)) = p.surrounding {
            self.field = Field {
                known: true,
                text,
                cursor: cursor as usize,
                anchor: anchor as usize,
            };
        }
        if let Some(purpose) = p.purpose {
            self.purpose = purpose;
        }
        if p.activate {
            self.active = true;
            self.log(&format!("activate purpose={:?}", self.purpose));
            self.events.push(UiEvent::Show {
                purpose: self.purpose,
            });
        } else if p.deactivate {
            self.active = false;
            self.field = Field::default();
            self.log("deactivate");
            self.events.push(UiEvent::Hide);
        }
    }
}

pub struct Wayland {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    im: ZwpInputMethodV2,
    vk: ZwpVirtualKeyboardV1,
    start: Instant,
    _seat: WlSeat,
    _im_manager: ZwpInputMethodManagerV2,
    _vk_manager: ZwpVirtualKeyboardManagerV1,
}

impl Wayland {
    fn connect(log: bool) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: WlSeat = globals
            .bind(&qh, 1..=9, ())
            .map_err(|_| "no seat".to_owned())?;
        let im_manager: ZwpInputMethodManagerV2 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| "the compositor does not offer input-method-v2".to_owned())?;
        let vk_manager: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| "the compositor does not offer virtual-keyboard-v1".to_owned())?;
        let im = im_manager.get_input_method(&seat, &qh, ());
        let vk = vk_manager.create_virtual_keyboard(&seat, &qh, ());
        set_keymap(&vk)?;
        vk.modifiers(0, 0, 0, 0);

        let mut state = State {
            log,
            serial: 0,
            active: false,
            purpose: Purpose::Normal,
            field: Field::default(),
            pending: Pending::default(),
            events: Vec::new(),
        };
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        state.log("connected");
        Ok(Self {
            conn,
            queue,
            state,
            im,
            vk,
            start: Instant::now(),
            _seat: seat,
            _im_manager: im_manager,
            _vk_manager: vk_manager,
        })
    }

    fn fd(&self) -> RawFd {
        self.conn.backend().poll_fd().as_raw_fd()
    }

    fn take_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.state.events)
    }

    fn flush(&self) {
        match self.conn.flush() {
            Ok(()) => {}
            Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => eprintln!("elavo-keyboard: flush failed: {e}"),
        }
    }

    /// Insert text at the cursor of the focused field.
    fn commit_text(&mut self, text: &str) {
        if !self.state.active {
            self.state.log("text with no active field, dropped");
            return;
        }
        self.im.commit_string(text.to_owned());
        self.im.commit(self.state.serial);
        self.state.field.insert(text);
        self.state.log(&format!("commit {text:?}"));
        self.flush();
    }

    /// Delete one character. Uses delete_surrounding_text when the field
    /// told us its text, and a Backspace key otherwise (no surrounding text,
    /// a selection, or the start of a paragraph in a multi-line field).
    fn backspace(&mut self) {
        if self.state.active {
            if let Some(len) = self.state.field.char_before() {
                self.im.delete_surrounding_text(len as u32, 0);
                self.im.commit(self.state.serial);
                let f = &mut self.state.field;
                f.text.replace_range(f.cursor - len..f.cursor, "");
                f.cursor -= len;
                f.anchor = f.cursor;
                self.state.log("delete 1 before cursor");
                self.flush();
                return;
            }
        }
        self.state.log("backspace key");
        self.tap_key(keys::BACKSPACE);
    }

    /// Press and release one evdev key on the virtual keyboard.
    fn tap_key(&mut self, code: u32) {
        let time = self.start.elapsed().as_millis() as u32;
        self.vk.key(time, code, 1);
        self.vk.key(time.wrapping_add(1), code, 0);
        self.flush();
    }
}

/// An edit for the input-method thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Text(String),
    Backspace,
    Key(u32),
}

/// Sends edits to the input-method thread and wakes it.
pub struct Commands {
    tx: mpsc::Sender<Command>,
    wake: OwnedFd,
}

impl Commands {
    pub fn send(&self, command: Command) {
        if self.tx.send(command).is_ok() {
            let byte = 1u8;
            unsafe { libc::write(self.wake.as_raw_fd(), (&byte as *const u8).cast(), 1) };
        }
    }
}

/// Connect on a new thread. Fails here if the compositor lacks a protocol.
pub fn spawn(log: bool) -> Result<(Commands, async_channel::Receiver<UiEvent>), String> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(format!("pipe: {}", std::io::Error::last_os_error()));
    }
    let (read_end, write_end) =
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("input-method".into())
        .spawn(move || {
            let mut wl = match Wayland::connect(log) {
                Ok(wl) => {
                    let _ = ready_tx.send(Ok(()));
                    wl
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            if let Err(e) = wl.run(&read_end, &cmd_rx, &ev_tx) {
                eprintln!("elavo-keyboard: {e}");
            }
            let _ = ev_tx.send_blocking(UiEvent::Unavailable);
        })
        .map_err(|e| e.to_string())?;
    ready_rx
        .recv()
        .map_err(|_| "input-method thread exited".to_owned())??;
    Ok((
        Commands {
            tx: cmd_tx,
            wake: write_end,
        },
        ev_rx,
    ))
}

fn poll2(a: RawFd, b: RawFd) -> std::io::Result<(bool, bool)> {
    let mut fds = [
        libc::pollfd {
            fd: a,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: b,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        let hit =
            |f: &libc::pollfd| f.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
        return Ok((hit(&fds[0]), hit(&fds[1])));
    }
}

impl Wayland {
    fn run(
        &mut self,
        wake: &OwnedFd,
        commands: &mpsc::Receiver<Command>,
        events: &async_channel::Sender<UiEvent>,
    ) -> Result<(), String> {
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(|e| e.to_string())?;
            for event in self.take_events() {
                let gone = event == UiEvent::Unavailable;
                if events.send_blocking(event).is_err() || gone {
                    return Ok(());
                }
            }
            self.flush();
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let (wl_ready, cmd_ready) =
                poll2(self.fd(), wake.as_raw_fd()).map_err(|e| e.to_string())?;
            if wl_ready {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.to_string()),
                }
            } else {
                drop(guard);
            }
            if cmd_ready {
                let mut buf = [0u8; 64];
                while unsafe { libc::read(wake.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) }
                    > 0
                {}
                while let Ok(command) = commands.try_recv() {
                    match command {
                        Command::Text(text) => self.commit_text(&text),
                        Command::Backspace => self.backspace(),
                        Command::Key(code) => self.tap_key(code),
                    }
                }
            }
        }
    }
}

fn set_keymap(vk: &ZwpVirtualKeyboardV1) -> Result<(), String> {
    let name = c"elavo-keyboard-keymap";
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = KEYMAP.as_bytes().to_vec();
    bytes.push(0);
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    // 1 is WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1.
    vk.keymap(1, file.as_fd(), bytes.len() as u32);
    Ok(())
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

impl Dispatch<ZwpInputMethodManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpInputMethodManagerV2,
        _: <ZwpInputMethodManagerV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, ZwpInputMethodManagerV2, [
        crate::protocol::input_method::zwp_input_method_manager_v2::REQ_GET_INPUT_METHOD_OPCODE => (ZwpInputMethodV2, ()),
    ]);
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

    event_created_child!(State, ZwpVirtualKeyboardManagerV1, [
        crate::protocol::virtual_keyboard::zwp_virtual_keyboard_manager_v1::REQ_CREATE_VIRTUAL_KEYBOARD_OPCODE => (ZwpVirtualKeyboardV1, ()),
    ]);
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

impl Dispatch<ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: <ZwpInputMethodV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ImEvent::Activate => {
                state.pending = Pending {
                    activate: true,
                    ..Pending::default()
                };
            }
            ImEvent::Deactivate => {
                state.pending.deactivate = true;
                state.pending.activate = false;
            }
            ImEvent::SurroundingText {
                text,
                cursor,
                anchor,
            } => state.pending.surrounding = Some((text, cursor, anchor)),
            ImEvent::ContentType { purpose, .. } => {
                state.pending.purpose = Some(purpose_of(purpose));
            }
            ImEvent::TextChangeCause { .. } => {}
            ImEvent::Done => state.apply_done(),
            ImEvent::Unavailable => {
                state.log("unavailable");
                state.events.push(UiEvent::Unavailable);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str, cursor: usize) -> Field {
        Field {
            known: true,
            text: text.into(),
            cursor,
            anchor: cursor,
        }
    }

    #[test]
    fn deletes_whole_characters() {
        assert_eq!(field("ab", 2).char_before(), Some(1));
        assert_eq!(field("a\u{e9}", 3).char_before(), Some(2));
        assert_eq!(field("ab", 0).char_before(), None);
    }

    #[test]
    fn selection_falls_back_to_the_key() {
        let mut f = field("abc", 3);
        f.anchor = 1;
        assert_eq!(f.char_before(), None);
    }

    #[test]
    fn insert_replaces_selection() {
        let mut f = field("abc", 3);
        f.anchor = 1;
        f.insert("Z");
        assert_eq!((f.text.as_str(), f.cursor), ("aZ", 2));
    }
}
