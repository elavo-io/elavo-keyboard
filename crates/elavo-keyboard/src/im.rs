//! The input-method thread: the seat's one input-method-v2 object, its
//! keyboard grab, and a virtual-keyboard-v1 device.
//!
//! Physical keys work as they did in Folio's `elavo-input`, which this
//! replaces: labwc hands this process the keyboard, and every key is sent
//! back through the virtual keyboard, except the second space of a quick pair
//! and the Backspace that undoes it. Those become text commits. The rule is
//! in `period::decide`.
//!
//! On-screen keys arrive as [`Command`]s. Text is committed with
//! `commit_string`, and delete uses `delete_surrounding_text` when the field
//! reports its text. Return, and delete without surrounding text, are keys
//! on the virtual keyboard. On-screen spaces go through the same period rule.
//!
//! This runs on its own thread and Wayland connection. [`spawn`] returns a
//! [`Commands`] handle and a channel of [`UiEvent`]s for the GTK side.

use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wayland_client::backend::WaylandError;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_keyboard::{KeyState, KeymapFormat};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{
    event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};

use crate::config;
use crate::period::config::{load_path, Watch};
use crate::period::decide::{
    self, decide, substitution_still_there, Action, Echo, Field, Input, Memory, Snapshot,
    KEY_BACKSPACE, KEY_SPACE,
};
use crate::protocol::input_method::zwp_input_method_keyboard_grab_v2::{
    Event as GrabEvent, ZwpInputMethodKeyboardGrabV2,
};
use crate::protocol::input_method::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use crate::protocol::input_method::zwp_input_method_v2::{Event as ImEvent, ZwpInputMethodV2};
use crate::protocol::input_method::zwp_text_input_v3::ContentPurpose;
use crate::protocol::virtual_keyboard::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use crate::protocol::virtual_keyboard::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

/// Linux evdev key codes the on-screen keys send through virtual-keyboard.
/// The arrows are for the cursor keys of a later layout.
#[allow(dead_code)]
pub mod keys {
    pub const BACKSPACE: u32 = 14;
    pub const ENTER: u32 = 28;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const UP: u32 = 103;
    pub const DOWN: u32 = 108;
}

/// Evdev codes that do not repeat into text.
const MODIFIERS: &[u32] = &[29, 42, 54, 56, 58, 69, 97, 100, 125, 126];

/// Until the grab reports the physical keyboard's keymap (or when there is
/// no physical keyboard), on-screen Return and delete need one. The
/// compositor compiles it with the system XKB data.
const FALLBACK_KEYMAP: &str = "xkb_keymap {\n\
    xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n\
    xkb_types { include \"complete\" };\n\
    xkb_compat { include \"complete\" };\n\
    xkb_symbols { include \"pc+us+inet(evdev)\" };\n\
};\n";

/// What the UI should know after a dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// A text field took focus. `purpose` is the text-input-v3 content purpose.
    Show { purpose: u32 },
    /// No text field has focus.
    Hide,
    /// A physical key went down.
    PhysicalKey,
    /// `onscreen-keyboard.conf` changed.
    Config(config::Config),
    /// Another input method has the seat, or the connection is gone.
    Unavailable,
}

/// An edit from the on-screen keys.
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
pub fn spawn(
    log: bool,
    osk_config: config::Config,
) -> Result<(Commands, async_channel::Receiver<UiEvent>), String> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(format!("pipe: {}", io::Error::last_os_error()));
    }
    let (read_end, write_end) =
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("input-method".into())
        .spawn(move || {
            let mut wl = match Wayland::connect(log, osk_config) {
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

struct Repeat {
    key: u32,
    next: Instant,
    time: u32,
    first: bool,
}

#[derive(Default)]
struct Pending {
    activate: bool,
    deactivate: bool,
    surrounding: Option<Snapshot>,
    purpose: Option<u32>,
}

impl Pending {
    fn clear_text(&mut self) {
        self.surrounding = None;
        self.purpose = None;
    }
}

struct State {
    im: Option<ZwpInputMethodV2>,
    vk: Option<ZwpVirtualKeyboardV1>,

    keymap_set: bool,
    serial: u32,
    active: bool,
    /// Add period with double-space.
    enabled: bool,
    purpose: u32,
    modifiers: bool,
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
    rate: i32,
    delay: i32,
    field: Field,
    memory: Memory,
    pending: Pending,
    swallowed: Vec<u32>,
    repeat: Option<Repeat>,
    period_path: Option<std::path::PathBuf>,
    osk_path: Option<std::path::PathBuf>,
    mtimes: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    osk_config: config::Config,
    watch: Option<Watch>,
    events: Vec<UiEvent>,
    dead: bool,
    trace: bool,
}

pub struct Wayland {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    _grab: ZwpInputMethodKeyboardGrabV2,
    _seat: WlSeat,
    _im_manager: ZwpInputMethodManagerV2,
    _vk_manager: ZwpVirtualKeyboardManagerV1,
}

impl Wayland {
    fn connect(trace: bool, osk_config: config::Config) -> Result<Self, String> {
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

        let period_path = crate::period::config::config_path();
        let osk_path = config::path();
        let watch_on = period_path.clone().or_else(|| osk_path.clone());
        let mut state = State {
            im: None,
            vk: None,
            keymap_set: false,
            serial: 0,
            active: false,
            enabled: period_path.as_deref().map(load_path).unwrap_or(true),
            purpose: 0,
            modifiers: false,
            depressed: 0,
            latched: 0,
            locked: 0,
            group: 0,
            rate: 0,
            delay: 0,
            field: Field::default(),
            memory: Memory::default(),
            pending: Pending::default(),
            swallowed: Vec::new(),
            repeat: None,
            mtimes: (
                period_path.as_deref().and_then(mtime),
                osk_path.as_deref().and_then(mtime),
            ),
            period_path,
            osk_path,
            osk_config,
            watch: watch_on.as_deref().and_then(|p| Watch::open(p).ok()),
            events: Vec::new(),
            dead: false,
            trace,
        };

        let im = im_manager.get_input_method(&seat, &qh, ());
        let vk = vk_manager.create_virtual_keyboard(&seat, &qh, ());
        set_fallback_keymap(&vk)?;
        state.keymap_set = true;
        let grab = im.grab_keyboard(&qh, ());
        state.im = Some(im);
        state.vk = Some(vk);
        state.send_modifiers();
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        state.log(&format!(
            "ready, period {}",
            if state.enabled { "on" } else { "off" }
        ));
        Ok(Self {
            conn,
            queue,
            state,
            _grab: grab,
            _seat: seat,
            _im_manager: im_manager,
            _vk_manager: vk_manager,
        })
    }

    fn fd(&self) -> RawFd {
        self.conn.backend().poll_fd().as_raw_fd()
    }

    fn run(
        &mut self,
        wake_fd: &OwnedFd,
        commands: &mpsc::Receiver<Command>,
        events: &async_channel::Sender<UiEvent>,
    ) -> Result<(), String> {
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(|e| e.to_string())?;
            if self.state.dead {
                return Ok(());
            }
            for event in std::mem::take(&mut self.state.events) {
                if events.send_blocking(event).is_err() {
                    return Ok(());
                }
            }
            self.state.fire_repeat();

            let need_out = match self.conn.flush() {
                Ok(()) => false,
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => true,
                Err(e) => return Err(e.to_string()),
            };

            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let watch_fd = self.state.watch.as_ref().map(Watch::fd);
            let ready = wake(
                self.fd(),
                wake_fd.as_raw_fd(),
                watch_fd,
                self.state.repeat_timeout_ms(),
                need_out,
            )
            .map_err(|e| e.to_string())?;
            if ready.wayland {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.to_string()),
                }
            } else {
                drop(guard);
            }
            if ready.config {
                if self.state.watch.as_mut().is_some_and(Watch::changed) {
                    self.state.reload_config();
                }
            } else if self.state.watch.is_none() {
                self.state.poll_config();
            }
            if ready.commands {
                let mut buf = [0u8; 64];
                while unsafe { libc::read(wake_fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) }
                    > 0
                {}
                while let Ok(command) = commands.try_recv() {
                    match command {
                        Command::Text(text) => self.state.osk_text(&text),
                        Command::Backspace => self.state.osk_backspace(),
                        Command::Key(code) => self.state.osk_key(code),
                    }
                }
            }
        }
    }
}

struct Ready {
    wayland: bool,
    commands: bool,
    config: bool,
}

fn wake(
    wl: RawFd,
    commands: RawFd,
    watch: Option<RawFd>,
    timeout_ms: i32,
    need_out: bool,
) -> io::Result<Ready> {
    let mut fds = [
        libc::pollfd {
            fd: wl,
            events: libc::POLLIN | if need_out { libc::POLLOUT } else { 0 },
            revents: 0,
        },
        libc::pollfd {
            fd: commands,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: watch.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let count = if watch.is_some() { 3 } else { 2 };
    loop {
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), count, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        let hit =
            |f: &libc::pollfd| f.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
        return Ok(Ready {
            wayland: hit(&fds[0]),
            commands: hit(&fds[1]),
            config: count == 3 && fds[2].revents & libc::POLLIN != 0,
        });
    }
}

impl State {
    fn log(&self, msg: &str) {
        if self.trace {
            eprintln!("elavo-keyboard: {msg}");
        }
    }

    fn reload_config(&mut self) {
        if let Some(path) = &self.period_path {
            let enabled = load_path(path);
            if enabled != self.enabled {
                self.log(&format!(
                    "add period with double-space {}",
                    if enabled { "on" } else { "off" }
                ));
            }
            self.enabled = enabled;
            self.mtimes.0 = mtime(path);
        }
        if let Some(path) = &self.osk_path {
            self.mtimes.1 = mtime(path);
        }
        let osk = config::load();
        if osk != self.osk_config {
            self.log(&format!("on-screen keyboard {osk:?}"));
            self.osk_config = osk;
            self.events.push(UiEvent::Config(osk));
        }
    }

    /// Without inotify, look at the files' times about once a second.
    fn poll_config(&mut self) {
        let now = (
            self.period_path.as_deref().and_then(mtime),
            self.osk_path.as_deref().and_then(mtime),
        );
        if now != self.mtimes {
            self.reload_config();
        }
    }

    // Physical keys, as in elavo-input.

    fn on_key(&mut self, time: u32, key: u32, pressed: bool) {
        self.log(&format!(
            "key {key} down={pressed} active={} known={} cursor={}",
            self.active, self.field.known, self.field.current.cursor
        ));
        if !pressed {
            let swallowed = take(&mut self.swallowed, key);
            if self.repeat.as_ref().is_some_and(|repeat| repeat.key == key) {
                self.repeat = None;
            }
            if !swallowed {
                self.send_key(time, key, false);
            }
            return;
        }

        self.events.push(UiEvent::PhysicalKey);
        self.repeat = None;
        let outcome = decide(&self.input(time), key, true);
        self.memory = outcome.memory;
        match outcome.action {
            Action::Forward => {
                self.send_key(time, key, true);
                if outcome.note_space {
                    self.field.note_space();
                }
                self.arm_repeat(key, time);
            }
            Action::Replace { before, text } => {
                if self.commit_replace(before, text) {
                    self.swallow(key);
                    self.log(&format!("commit {text:?} deleting {before}"));
                } else {
                    self.send_key(time, key, true);
                    self.arm_repeat(key, time);
                }
            }
        }
    }

    fn input(&self, now: u32) -> Input<'_> {
        Input {
            enabled: self.enabled,
            active: self.active,
            purpose: self.purpose,
            modifiers: self.modifiers,
            text: &self.field.current.text,
            cursor: self.field.current.cursor,
            anchor: self.field.current.anchor,
            known: self.field.known,
            memory: &self.memory,
            now,
        }
    }

    fn commit_replace(&mut self, before: u32, text: &str) -> bool {
        if self.serial == 0 {
            return false;
        }
        let Some(im) = &self.im else {
            return false;
        };
        if before > 0 {
            im.delete_surrounding_text(before, 0);
        }
        im.commit_string(text.to_owned());
        im.commit(self.serial);
        self.field.replace(before, text);
        true
    }

    fn send_key(&self, time: u32, key: u32, pressed: bool) {
        if !self.keymap_set {
            return;
        }
        let Some(vk) = &self.vk else {
            return;
        };
        vk.key(time, key, u32::from(pressed));
    }

    fn send_modifiers(&self) {
        if !self.keymap_set {
            return;
        }
        let Some(vk) = &self.vk else {
            return;
        };
        vk.modifiers(self.depressed, self.latched, self.locked, self.group);
    }

    fn arm_repeat(&mut self, key: u32, time: u32) {
        if MODIFIERS.contains(&key) || self.rate <= 0 || self.delay < 0 {
            return;
        }
        self.repeat = Some(Repeat {
            key,
            next: Instant::now() + Duration::from_millis(self.delay as u64),
            time,
            first: true,
        });
    }

    fn fire_repeat(&mut self) {
        let (delay_ms, interval_ms) = repeat_steps(self.rate, self.delay);
        let Some(interval_ms) = interval_ms else {
            return;
        };
        let Some(repeat) = &self.repeat else {
            return;
        };
        if Instant::now() < repeat.next {
            return;
        }
        let key = repeat.key;
        let step = if repeat.first { delay_ms } else { interval_ms };
        let time = repeat.time.wrapping_add(step);
        if let Some(repeat) = &mut self.repeat {
            repeat.time = time;
            repeat.first = false;
            repeat.next = Instant::now() + Duration::from_millis(interval_ms as u64);
        }
        self.send_key(time, key, true);
    }

    fn repeat_timeout_ms(&self) -> i32 {
        let Some(repeat) = &self.repeat else {
            return if self.watch.is_some() { -1 } else { 1_000 };
        };
        let left = repeat.next.saturating_duration_since(Instant::now());
        left.as_millis().min(i32::MAX as u128) as i32
    }

    fn swallow(&mut self, key: u32) {
        if !self.swallowed.contains(&key) {
            self.swallowed.push(key);
        }
    }

    fn apply_done(&mut self) {
        self.serial = self.serial.saturating_add(1);
        let activate = self.pending.activate;
        let deactivate = self.pending.deactivate;
        if self.pending.activate {
            self.active = true;
            self.field.clear();
            self.memory = Memory::default();
        }
        if self.pending.deactivate {
            self.active = false;
            self.field.clear();
            self.memory = Memory::default();
            self.repeat = None;
        }
        if let Some(purpose) = self.pending.purpose.take() {
            self.purpose = purpose;
        } else if self.pending.activate {
            self.purpose = 0;
        }
        if let Some(snap) = self.pending.surrounding.take() {
            let echo = self.field.apply_remote(snap, true);
            self.note_echo(echo);
        } else if self.pending.activate {
            self.field.known = false;
        }
        self.pending.activate = false;
        self.pending.deactivate = false;
        if activate {
            self.log(&format!("activate purpose={}", self.purpose));
            self.events.push(UiEvent::Show {
                purpose: self.purpose,
            });
        } else if deactivate {
            self.log("deactivate");
            self.events.push(UiEvent::Hide);
        }
    }

    fn note_echo(&mut self, echo: Echo) {
        if echo == Echo::Stale {
            return;
        }
        let snap = &self.field.current;
        if self.memory.last_was_substitution
            && !substitution_still_there(&snap.text, snap.cursor, snap.anchor)
        {
            self.memory.last_was_substitution = false;
        }
    }

    fn on_keymap(&mut self, format: WEnum<KeymapFormat>, fd: OwnedFd, size: u32) {
        if let Some(vk) = &self.vk {
            vk.keymap(keymap_format(format), fd.as_fd(), size);
        }
        self.keymap_set = true;
        self.send_modifiers();
    }

    // On-screen keys.

    /// Milliseconds on the monotonic clock, the clock of the compositor's
    /// key times, so physical and on-screen spaces can pair up.
    fn osk_now(&self) -> u32 {
        monotonic_ms()
    }

    fn osk_text(&mut self, text: &str) {
        if !self.active || self.serial == 0 {
            self.log("on-screen text with no active field, dropped");
            return;
        }
        if text == " " {
            self.osk_space();
            return;
        }
        // Any other key ends a pending period or its undo, as on a keyboard.
        let outcome = decide(&self.input(self.osk_now()), 0, true);
        self.memory = outcome.memory;
        self.commit_text(text);
    }

    fn osk_space(&mut self) {
        let outcome = decide(&self.input(self.osk_now()), KEY_SPACE, true);
        self.memory = outcome.memory;
        match outcome.action {
            Action::Replace { before, text } if self.commit_replace(before, text) => {
                self.log(&format!("on-screen commit {text:?} deleting {before}"));
            }
            _ => self.commit_text(" "),
        }
    }

    fn commit_text(&mut self, text: &str) {
        let Some(im) = &self.im else {
            return;
        };
        im.commit_string(text.to_owned());
        im.commit(self.serial);
        self.field.replace(0, text);
        self.log(&format!("on-screen commit {text:?}"));
    }

    fn osk_backspace(&mut self) {
        if self.active && self.serial != 0 {
            let outcome = decide(&self.input(self.osk_now()), KEY_BACKSPACE, true);
            self.memory = outcome.memory;
            if let Action::Replace { before, text } = outcome.action {
                if self.commit_replace(before, text) {
                    self.log("on-screen delete undid the period");
                    return;
                }
            }
            if let Some(len) = char_before(&self.field) {
                if let Some(im) = &self.im {
                    im.delete_surrounding_text(len, 0);
                    im.commit(self.serial);
                    self.field.replace(len, "");
                    self.log("on-screen delete 1 before cursor");
                    return;
                }
            }
        }
        self.log("on-screen backspace key");
        self.osk_key(keys::BACKSPACE);
    }

    fn osk_key(&mut self, code: u32) {
        let time = self.osk_now();
        self.send_key(time, code, true);
        self.send_key(time.wrapping_add(1), code, false);
    }
}

/// Bytes of the character before the cursor, when the field reported its
/// text and nothing is selected.
fn char_before(field: &Field) -> Option<u32> {
    let s = &field.current;
    if !field.known || s.cursor != s.anchor || s.cursor == 0 {
        return None;
    }
    s.text
        .get(..s.cursor as usize)
        .and_then(|t| t.chars().next_back())
        .map(|c| c.len_utf8() as u32)
}

fn monotonic_ms() -> u32 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000) as u32
}

fn set_fallback_keymap(vk: &ZwpVirtualKeyboardV1) -> Result<(), String> {
    let name = c"elavo-keyboard-keymap";
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: {}", io::Error::last_os_error()));
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = FALLBACK_KEYMAP.as_bytes().to_vec();
    bytes.push(0);
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    // 1 is WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1.
    vk.keymap(1, file.as_fd(), bytes.len() as u32);
    Ok(())
}

fn repeat_steps(rate: i32, delay: i32) -> (u32, Option<u32>) {
    if rate <= 0 || delay < 0 {
        return (0, None);
    }
    let interval = (1000 / rate as u32).max(1);
    (delay as u32, Some(interval))
}

fn take(keys: &mut Vec<u32>, key: u32) -> bool {
    if let Some(index) = keys.iter().position(|held| *held == key) {
        keys.swap_remove(index);
        true
    } else {
        false
    }
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn keymap_format(format: WEnum<KeymapFormat>) -> u32 {
    match format {
        WEnum::Value(KeymapFormat::XkbV1) => 1,
        WEnum::Unknown(value) => value,
        WEnum::Value(_) => 0,
    }
}

fn is_pressed(state: WEnum<KeyState>) -> bool {
    match state {
        WEnum::Value(KeyState::Pressed) | WEnum::Unknown(1) => true,
        WEnum::Value(_) | WEnum::Unknown(_) => false,
    }
}

fn purpose_code(purpose: WEnum<ContentPurpose>) -> u32 {
    match purpose {
        WEnum::Value(ContentPurpose::Password) => decide::PURPOSE_PASSWORD,
        WEnum::Value(ContentPurpose::Pin) => decide::PURPOSE_PIN,
        WEnum::Value(ContentPurpose::Terminal) => decide::PURPOSE_TERMINAL,
        WEnum::Unknown(value) => value,
        WEnum::Value(_) => 0,
    }
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
                state.pending.activate = true;
                state.pending.deactivate = false;
                state.pending.clear_text();
            }
            ImEvent::Deactivate => {
                state.pending.deactivate = true;
                state.pending.activate = false;
            }
            ImEvent::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                state.pending.surrounding = Some(Snapshot {
                    text,
                    cursor,
                    anchor,
                });
            }
            ImEvent::TextChangeCause { .. } => {}
            ImEvent::ContentType { purpose, .. } => {
                state.pending.purpose = Some(purpose_code(purpose));
            }
            ImEvent::Done => state.apply_done(),
            ImEvent::Unavailable => {
                state.log("unavailable");
                state.dead = true;
            }
        }
    }

    event_created_child!(State, ZwpInputMethodV2, [
        crate::protocol::input_method::zwp_input_method_v2::REQ_GRAB_KEYBOARD_OPCODE => (ZwpInputMethodKeyboardGrabV2, ()),
    ]);
}

impl Dispatch<ZwpInputMethodKeyboardGrabV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodKeyboardGrabV2,
        event: <ZwpInputMethodKeyboardGrabV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            GrabEvent::Keymap { format, fd, size } => {
                state.log("keymap");
                state.on_keymap(format, fd, size);
            }
            GrabEvent::Key {
                time,
                key,
                state: key_state,
                ..
            } => {
                state.on_key(time, key, is_pressed(key_state));
            }
            GrabEvent::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state.depressed = mods_depressed;
                state.latched = mods_latched;
                state.locked = mods_locked;
                state.group = group;
                state.modifiers = mods_depressed != 0 || mods_latched != 0;
                state.send_modifiers();
            }
            GrabEvent::RepeatInfo { rate, delay } => {
                state.rate = rate;
                state.delay = delay;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str, cursor: u32) -> Field {
        let mut f = Field::default();
        f.apply_remote(
            Snapshot {
                text: text.into(),
                cursor,
                anchor: cursor,
            },
            true,
        );
        f
    }

    #[test]
    fn deletes_whole_characters() {
        assert_eq!(char_before(&field("ab", 2)), Some(1));
        assert_eq!(char_before(&field("a\u{e9}", 3)), Some(2));
        assert_eq!(char_before(&field("ab", 0)), None);
    }

    #[test]
    fn a_selection_falls_back_to_the_key() {
        let mut f = Field::default();
        f.apply_remote(
            Snapshot {
                text: "abc".into(),
                cursor: 3,
                anchor: 1,
            },
            true,
        );
        assert_eq!(char_before(&f), None);
    }
}
