//! Linux (X11) platform backend for MyKVM input sharing.
//!
//! The shared-input wire format is Windows virtual-key codes plus per-screen
//! layout coordinates, so the Linux side only ever translates at its own
//! edges:
//!
//! - capture (this machine controls a remote): X pointer grabs deliver every
//!   key/button/motion event to us while a remote screen is active (the grab
//!   also swallows them locally, exactly like the Windows low-level hooks do),
//!   and keycodes are translated to VK codes through the server's keyboard
//!   mapping;
//! - injection (this machine is controlled by a remote): XTEST fakes motion /
//!   buttons / keys, with VK codes translated back through the same mapping.
//!
//! Requirements: an X11 display (Xorg session, or XWayland on Wayland with
//! reduced global-grab fidelity). XTEST is required for injection, XFIXES is
//! used to hide the local cursor while a remote screen is active (best-effort:
//! capture keeps working without it). Pure-Wayland sessions have no X display
//! and are rejected with a clear error by [`probe`]/[`X11::open`].
#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{self, ConnectionExt as XProtoExt, EventMask, GrabMode};
use x11rb::protocol::{xfixes, xtest, Event};
use x11rb::rust_connection::RustConnection;

use crate::shared_input::MouseButton;

type LinuxResult<T> = Result<T, String>;

/// How long a fetched keyboard mapping may be reused before we ask the server
/// again. Layout switches (e.g. XKB group changes) must not leave the capture
/// or injection translation stale for long.
const KEYMAP_TTL: std::time::Duration = std::time::Duration::from_millis(1000);

/// What the environment probe found, so callers can explain failures well.
#[derive(Debug, Clone)]
pub struct X11Probe {
    /// XTEST extension present (needed to inject synthetic input).
    pub xtest_available: bool,
    /// Whether a Wayland session env var is visible alongside DISPLAY, which
    /// usually means XWayland (global grabs work with reduced fidelity).
    pub xwayland_hint: bool,
}


/// Opens an X11 connection and reports extension availability without keeping
/// the connection. Failures carry a human-readable reason.
pub fn probe() -> LinuxResult<X11Probe> {
    let x11 = X11::open_aux(false)?;
    Ok(X11Probe {
        xtest_available: x11.extension_present(xtest::X11_EXTENSION_NAME),
        xwayland_hint: !std::env::var("WAYLAND_DISPLAY").unwrap_or_default().is_empty(),
    })
}

/// Cached keyboard mapping for one server connection: the per-keycode keysym
/// rows returned by GetKeyboardMapping. With XKB enabled (the norm) the row
/// for every keycode is `per` syms long and group 0 occupies the first two
/// positions (level 0 = unmodified, level 1 = Shift/Caps). Group 1+ (AltGr /
/// input-method layouts with more rows) is intentionally not resolved: those
/// keysyms are only used as a fallback when group 0 yields nothing.
struct Keymap {
    fetched_at: Instant,
    min_keycode: u8,
    /// Keysyms per keycode as reported by the server.
    per: usize,
    /// Flat keysym rows, row `keycode - min_keycode` at
    /// `(keycode - min_keycode) * per`.
    syms: Vec<u32>,
}

/// A live connection to the X server. All requests are `&self` (x11rb
/// connections synchronize internally), so one instance can be shared by the
/// capture thread; the injector keeps its own lazily-opened instance.
pub struct X11 {
    conn: RustConnection,
    root: xproto::Window,
    keymap: Mutex<Option<Keymap>>,
    cursor_hidden: AtomicBool,
}

impl X11 {
    /// Opens the default display. `require_xtest` is true for the injection
    /// side (synthetic input is impossible without XTEST).
    pub fn open(require_xtest: bool) -> LinuxResult<Self> {
        Self::open_aux(require_xtest)
    }

    fn open_aux(require_xtest: bool) -> LinuxResult<Self> {
        let (conn, screen_num) = RustConnection::connect(None)
            .map_err(|error| format!("failed to connect to X11 display: {error}"))?;
        let setup = conn.setup();
        let root = setup.roots[screen_num].root;
        let x11 = Self {
            conn,
            root,
            keymap: Mutex::new(None),
            cursor_hidden: AtomicBool::new(false),
        };
        if require_xtest && !x11.extension_present(xtest::X11_EXTENSION_NAME) {
            return Err(format!(
                "X server at {} has no XTEST extension — it cannot inject synthetic input",
                std::env::var("DISPLAY").unwrap_or_default()
            ));
        }
        Ok(x11)
    }

    /// True when the named extension is usable on this server.
    pub fn extension_present(&self, name: &'static str) -> bool {
        self.conn
            .extension_information(name)
            .ok()
            .flatten()
            .is_some()
    }

    // ------------------------------------------------------------------
    // Pointer
    // ------------------------------------------------------------------

    /// Current pointer position in root coordinates (native/physical pixels).
    pub fn query_pointer(&self) -> Option<(f64, f64)> {
        let reply = xproto::query_pointer(&self.conn, self.root).ok()?.reply().ok()?;
        Some((reply.root_x as f64, reply.root_y as f64))
    }

    /// Warps the pointer to root coordinates.
    pub fn warp_pointer(&self, x: f64, y: f64) {
        let _ = xproto::warp_pointer(
            &self.conn,
            0u32, // no source window: warp anywhere
            self.root,
            0,
            0,
            0,
            0,
            x.clamp(i16::MIN as f64, i16::MAX as f64) as i16,
            y.clamp(i16::MIN as f64, i16::MAX as f64) as i16,
        );
        let _ = self.conn.flush();
    }

    /// Grabs both the pointer and the keyboard for this connection. While the
    /// grabs are active every pointer/keyboard event is delivered to us and no
    /// other client sees them — the local equivalent of the Windows hooks
    /// swallowing input while a remote screen is active.
    ///
    /// Returns an error (and releases any partial grab) if either grab fails,
    /// e.g. because another client already holds one.
    pub fn grab_pointer_and_keyboard(&self) -> LinuxResult<()> {
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let pointer = xproto::grab_pointer(
            &self.conn,
            false, // owner_events: only us see events while grabbed
            self.root,
            mask,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            0u32, // no confine
            0u32, // no cursor override
            0u32,
        )
        .map_err(|error| format!("grab pointer request failed: {error}"))?
        .reply()
        .map_err(|error| format!("grab pointer reply failed: {error}"))?;
        if pointer.status != xproto::GrabStatus::SUCCESS {
            let _ = self.ungrab_pointer_and_keyboard();
            return Err(format!("pointer grab refused: {:?}", pointer.status));
        }

        let keyboard = xproto::grab_keyboard(
            &self.conn,
            false,
            self.root,
            0u32,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .map_err(|error| format!("grab keyboard request failed: {error}"))?
        .reply()
        .map_err(|error| format!("grab keyboard reply failed: {error}"))?;
        if keyboard.status != xproto::GrabStatus::SUCCESS {
            let _ = self.ungrab_pointer_and_keyboard();
            return Err(format!("keyboard grab refused: {:?}", keyboard.status));
        }
        let _ = self.conn.flush();
        Ok(())
    }

    pub fn ungrab_pointer_and_keyboard(&self) -> LinuxResult<()> {
        let _ = xproto::ungrab_pointer(&self.conn, 0u32);
        let _ = xproto::ungrab_keyboard(&self.conn, 0u32);
        let _ = self.conn.flush();
        Ok(())
    }

    /// Hides the cursor over the root window (XFIXES). Best-effort: when the
    /// extension is missing the cursor stays visible but capture still works.
    pub fn hide_cursor(&self) {
        if self.cursor_hidden.load(Ordering::Relaxed) {
            return;
        }
        match xfixes::hide_cursor(&self.conn, self.root) {
            Ok(_) => {
                let _ = self.conn.flush();
                self.cursor_hidden.store(true, Ordering::Relaxed);
            }
            Err(error) => {
                log::warn!("hide cursor failed ({error}); cursor stays visible while remote control is active");
            }
        }
    }

    pub fn show_cursor(&self) {
        if !self.cursor_hidden.load(Ordering::Relaxed) {
            return;
        }
        let _ = xfixes::show_cursor(&self.conn, self.root);
        let _ = self.conn.flush();
        self.cursor_hidden.store(false, Ordering::Relaxed);
    }

    /// Non-blocking read of one queued event.
    pub fn poll_event(&self) -> Option<Event> {
        self.conn.poll_for_event().ok().flatten()
    }

    /// Keycodes of every key that is physically down right now (QueryKeymap).
    pub fn pressed_keycodes(&self) -> Vec<u8> {
        let setup = self.conn.setup();
        let min = setup.min_keycode;
        let max = setup.max_keycode;
        let Some(reply) = xproto::query_keymap(&self.conn)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            return Vec::new();
        };
        reply
            .keys
            .iter()
            .enumerate()
            .flat_map(|(byte_index, byte)| {
                (0..8).filter_map(move |bit| {
                    let keycode = byte_index as u16 * 8 + bit as u16;
                    if byte & (1 << bit) != 0 && (min as u16..=max as u16).contains(&keycode) {
                        u8::try_from(keycode).ok()
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Keyboard mapping and key translation
    // ------------------------------------------------------------------

    fn refresh_keymap(&self) -> Option<()> {
        if let Ok(keymap) = self.keymap.lock() {
            if keymap
                .as_ref()
                .map(|keymap| keymap.fetched_at.elapsed() < KEYMAP_TTL)
                .unwrap_or(false)
            {
                return Some(());
            }
        }

        let setup = self.conn.setup();
        let min_keycode = setup.min_keycode;
        let keycode_count = u8::try_from(setup.max_keycode - setup.min_keycode + 1).ok()?;
        let reply = self
            .conn
            .get_keyboard_mapping(min_keycode, keycode_count)
            .ok()?
            .reply()
            .ok()?;
        let fetched = Keymap {
            fetched_at: Instant::now(),
            min_keycode,
            per: reply.keysyms_per_keycode.max(1) as usize,
            syms: reply.keysyms,
        };
        if let Ok(mut keymap) = self.keymap.lock() {
            *keymap = Some(fetched);
        }
        Some(())
    }

    /// The keysym produced by `keycode` under modifier mask `state` (the core
    /// event `state` field: bit 0 = Shift, bit 1 = Caps Lock). Falls back to
    /// group 0 level 0 when the shift level is empty.
    pub fn effective_keysym(&self, keycode: u8, state: u16) -> u32 {
        self.refresh_keymap();
        let Ok(guard) = self.keymap.lock() else {
            return 0;
        };
        let Some(keymap) = guard.as_ref() else {
            return 0;
        };
        let row_index = (keycode as i32 - keymap.min_keycode as i32) as usize;
        let Some(row) = keymap.syms.get(row_index * keymap.per..(row_index + 1) * keymap.per)
        else {
            return 0;
        };
        if row.is_empty() {
            return 0;
        }

        // XKB servers report group 0 first: levels 0 (plain) and 1 (Shift).
        // Caps Lock shifts letters only.
        let level0_is_letter = matches!(row[0], 0x41..=0x5A | 0x61..=0x7A);
        let shifted = state & 0x0001 != 0 || (state & 0x0002 != 0 && level0_is_letter);
        let index = if shifted && row.len() > 1 { 1 } else { 0 };
        let sym = row[index];
        if sym != 0 {
            return sym;
        }
        // Defensive: an empty level should never hide a mapped level 0.
        row.iter().copied().find(|sym| *sym != 0).unwrap_or(0)
    }

    /// Finds a keycode that produces `keysym` in the current mapping (scanning
    /// every reported level, like XKeysymToKeycode).
    pub fn keycode_for_keysym(&self, keysym: u32) -> Option<u8> {
        self.refresh_keymap();
        let keymap = self.keymap.lock().ok()?;
        let keymap = keymap.as_ref()?;
        let rows = keymap.syms.len() / keymap.per;
        for row in 0..rows {
            let slice = &keymap.syms[row * keymap.per..(row + 1) * keymap.per];
            if slice.contains(&keysym) {
                let keycode = keymap.min_keycode as u16 + row as u16;
                return u8::try_from(keycode).ok();
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // XTEST injection (used by the receive side; also reachable from the
    // capture thread's own connection, harmless either way)
    // ------------------------------------------------------------------

    fn fake_input(&self, type_: u8, detail: u8, root_x: i16, root_y: i16) {
        let _ = xtest::fake_input(
            &self.conn,
            type_,
            detail,
            0,
            self.root,
            root_x,
            root_y,
            0,
        );
    }

    /// Absolute pointer move in root coordinates.
    pub fn fake_motion(&self, x: i32, y: i32) {
        self.fake_input(
            xproto::MOTION_NOTIFY_EVENT,
            0,
            x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        );
        let _ = self.conn.flush();
    }

    pub fn fake_button(&self, button: u8, down: bool) {
        let type_ = if down {
            xproto::BUTTON_PRESS_EVENT
        } else {
            xproto::BUTTON_RELEASE_EVENT
        };
        self.fake_input(type_, button, 0, 0);
        let _ = self.conn.flush();
    }

    pub fn fake_key(&self, keycode: u8, down: bool) {
        let type_ = if down {
            xproto::KEY_PRESS_EVENT
        } else {
            xproto::KEY_RELEASE_EVENT
        };
        self.fake_input(type_, keycode, 0, 0);
        let _ = self.conn.flush();
    }
}

/// X11 button numbers (core protocol): 1-3 are left/middle/right, 4-7 the two
/// wheel axes, 8/9 the back/forward side buttons (evdev conventions).
fn x11_button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    }
}

// ----------------------------------------------------------------------
// Keysym <-> Windows VK translation
//
// The wire format is Windows VK codes, and this table mirrors what
// `mac_key_to_windows_vk` / `windows_vk_to_mac_key` do for macOS: VK is the
// *physical key identity* (letters and digits never depend on Shift; the
// shifted punctuation pairs share one VK). Capture resolves the effective
// keysym from the server mapping first, so shifted keysyms ('!' instead of
// '1') still land on the right VK. The unshifted "base" keysym is used for
// injection so a VK arrives at the key that produces that character in the
// receiving layout — character semantics, the same approach barrier uses.
// ----------------------------------------------------------------------

/// Keysym constants used by the tables below (values from keysymdef.h).
mod ks {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const ISO_LEFT_TAB: u32 = 0xfe20;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const DELETE: u32 = 0xffff;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const INSERT: u32 = 0xff63;
    pub const MENU: u32 = 0xff67;
    pub const PRINT: u32 = 0xff61;
    pub const SYS_REQ: u32 = 0xff15;
    pub const PAUSE: u32 = 0xff13;
    pub const SCROLL_LOCK: u32 = 0xff14;
    pub const NUM_LOCK: u32 = 0xff7f;
    pub const CAPS_LOCK: u32 = 0xffe5;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const META_L: u32 = 0xffe7;
    pub const META_R: u32 = 0xffe8;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const SUPER_L: u32 = 0xffeb;
    pub const SUPER_R: u32 = 0xffec;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const KP_MULTIPLY: u32 = 0xffaa;
    pub const KP_ADD: u32 = 0xffab;
    pub const KP_SUBTRACT: u32 = 0xffad;
    pub const KP_DECIMAL: u32 = 0xffae;
    pub const KP_DIVIDE: u32 = 0xffaf;
    pub const KP_INSERT: u32 = 0xff9e;
    pub const KP_DELETE: u32 = 0xff9f;
    pub const KP_HOME: u32 = 0xff95;
    pub const KP_UP: u32 = 0xff97;
    pub const KP_PAGE_UP: u32 = 0xff9a;
    pub const KP_LEFT: u32 = 0xff96;
    pub const KP_BEGIN: u32 = 0xff9d;
    pub const KP_RIGHT: u32 = 0xff98;
    pub const KP_END: u32 = 0xff9c;
    pub const KP_DOWN: u32 = 0xff99;
    pub const KP_PAGE_DOWN: u32 = 0xff9b;
}

/// Maps an X11 keysym to the Windows VK code of the physical key. Returns
/// `None` for keys with no VK counterpart (media keys, dead keys, AltGr-only
/// symbols, ...) — the caller drops the event and logs at debug level, same as
/// the macOS capture path does for unmapped keycodes.
pub fn keysym_to_windows_vk(sym: u32) -> Option<u16> {
    // Letters: any case normalizes to the same VK.
    match sym {
        0x41..=0x5A => return Some(sym as u16),
        0x61..=0x7A => return Some(sym as u16 - 0x20),
        _ => {}
    }
    // ASCII digits and space map 1:1.
    match sym {
        0x30..=0x39 => return Some(sym as u16),
        0x20 => return Some(0x20),
        _ => {}
    }

    let vk = match sym {
        ks::BACKSPACE => 0x08,
        ks::TAB | ks::ISO_LEFT_TAB => 0x09,
        ks::RETURN => 0x0D,
        ks::ESCAPE => 0x1B,
        ks::INSERT => 0x2D,
        ks::DELETE => 0x2E,
        ks::HOME => 0x24,
        ks::END => 0x23,
        ks::PAGE_UP => 0x21,
        ks::PAGE_DOWN => 0x22,
        ks::LEFT => 0x25,
        ks::UP => 0x26,
        ks::RIGHT => 0x27,
        ks::DOWN => 0x28,
        ks::PRINT | ks::SYS_REQ => 0x2C, // PrintScreen
        ks::PAUSE => 0x13,
        ks::SCROLL_LOCK => 0x91,
        ks::NUM_LOCK => 0x90,
        ks::CAPS_LOCK => 0x14,
        ks::MENU => 0x5D, // Application key
        ks::SHIFT_L => 0xA0,
        ks::SHIFT_R => 0xA1,
        ks::CONTROL_L => 0xA2,
        ks::CONTROL_R => 0xA3,
        ks::ALT_L => 0xA4,
        ks::ALT_R => 0xA5,
        ks::SUPER_L | ks::META_L => 0x5B,
        ks::SUPER_R | ks::META_R => 0x5C,
        // Shifted ASCII punctuation (both members of a US pair share a VK).
        0x21 => 0x31, // ! on the 1 key
        0x40 => 0x32, // @
        0x23 => 0x33, // #
        0x24 => 0x34, // $
        0x25 => 0x35, // %
        0x5E => 0x36, // ^
        0x26 => 0x37, // &
        0x2A => 0x38, // *
        0x28 => 0x39, // (
        0x29 => 0x30, // )
        0x3B | 0x3A => 0xBA, // ; :
        0x2B | 0x3D => 0xBB, // + =
        0x2C | 0x3C => 0xBC, // , <
        0x2D | 0x5F => 0xBD, // - _
        0x2E | 0x3E => 0xBE, // . >
        0x2F | 0x3F => 0xBF, // / ?
        0x60 | 0x7E => 0xC0, // ` ~
        0x5B | 0x7B => 0xDB, // [ {
        0x5C | 0x7C => 0xDC, // \ |
        0x5D | 0x7D => 0xDD, // ] }
        0x27 | 0x22 => 0xDE, // ' "
        // Keypad digits (NumLock on).
        0xFFB0..=0xFFB9 => 0x60 + (sym - 0xFFB0) as u16,
        ks::KP_DECIMAL => 0x6E,
        ks::KP_DIVIDE => 0x6F,
        ks::KP_MULTIPLY => 0x6A,
        ks::KP_SUBTRACT => 0x6D,
        ks::KP_ADD => 0x6B,
        ks::KP_ENTER => 0x0D,
        // Keypad navigation (NumLock off, Windows reports the nav VK).
        ks::KP_HOME => 0x24,
        ks::KP_UP => 0x26,
        ks::KP_PAGE_UP => 0x21,
        ks::KP_LEFT => 0x25,
        ks::KP_BEGIN => 0x0C, // VK_CLEAR (keypad 5)
        ks::KP_RIGHT => 0x27,
        ks::KP_END => 0x23,
        ks::KP_DOWN => 0x28,
        ks::KP_PAGE_DOWN => 0x22,
        ks::KP_INSERT => 0x2D,
        ks::KP_DELETE => 0x2E,
        // Function keys F1..F24.
        0xFFBE..=0xFFD5 => 0x70 + (sym - 0xFFBE) as u16,
        _ => return None,
    };
    Some(vk)
}

/// Base (unshifted, US-reference) keysym for a VK code — the key we press on
/// the receiving machine to produce that key's character. Returns `None` for
/// VKs with no X11 equivalent (media keys etc.).
pub fn windows_vk_to_keysym(vk: u16) -> Option<u32> {
    // Letters arrive as VK_A..VK_Z and must type the *lowercase* keysym: the
    // shift level is conveyed by the separately forwarded Shift key, so this
    // keycode types 'a' normally and 'A' while Shift is held on the target.
    match vk {
        0x30..=0x39 => return Some(vk as u32),
        0x41..=0x5A => return Some(vk as u32 + 0x20),
        0x20 => return Some(0x20),
        _ => {}
    }
    let sym = match vk {
        0x08 => ks::BACKSPACE,
        0x09 => ks::TAB,
        0x0D => ks::RETURN,
        0x1B => ks::ESCAPE,
        0x14 => ks::CAPS_LOCK,
        0x21 => ks::PAGE_UP,
        0x22 => ks::PAGE_DOWN,
        0x23 => ks::END,
        0x24 => ks::HOME,
        0x25 => ks::LEFT,
        0x26 => ks::UP,
        0x27 => ks::RIGHT,
        0x28 => ks::DOWN,
        0x2C => ks::PRINT,
        0x2D => ks::INSERT,
        0x2E => ks::DELETE,
        0x5B => ks::SUPER_L,
        0x5C => ks::SUPER_R,
        0x5D => ks::MENU,
        0x13 => ks::PAUSE,
        0x90 => ks::NUM_LOCK,
        0x91 => ks::SCROLL_LOCK,
        0xA0 => ks::SHIFT_L,
        0xA1 => ks::SHIFT_R,
        0xA2 => ks::CONTROL_L,
        0xA3 => ks::CONTROL_R,
        0xA4 => ks::ALT_L,
        0xA5 => ks::ALT_R,
        0x10 => ks::SHIFT_L,
        0x11 => ks::CONTROL_L,
        0x12 => ks::ALT_L,
        // Main-row punctuation: the base (unshifted) US keysym.
        0xBA => 0x3B, // ;
        0xBB => 0x3D, // =
        0xBC => 0x2C, // ,
        0xBD => 0x2D, // -
        0xBE => 0x2E, // .
        0xBF => 0x2F, // /
        0xC0 => 0x60, // `
        0xDB => 0x5B, // [
        0xDC => 0x5C, // backslash
        0xDD => 0x5D, // ]
        0xDE => 0x27, // '
        // Keypad.
        0x60..=0x69 => 0xFFB0 + (vk - 0x60) as u32,
        0x6E => ks::KP_DECIMAL,
        0x6F => ks::KP_DIVIDE,
        0x6A => ks::KP_MULTIPLY,
        0x6B => ks::KP_ADD,
        0x6D => ks::KP_SUBTRACT,
        // Function keys F1..F24.
        0x70..=0x87 => 0xFFBE + (vk - 0x70) as u32,
        _ => return None,
    };
    Some(sym)
}

// ----------------------------------------------------------------------
// Injection (receive side). Mirrors windows_input.rs: a lazily opened X
// connection plus pressed-state tracking so ReleaseAll can always unstick
// keys and buttons. All calls are safe from any thread (QUIC datagram
// workers); a mutex serializes actual X traffic.
// ----------------------------------------------------------------------

static INJECTOR: Mutex<Option<X11>> = Mutex::new(None);
static PRESSED_KEYS: Mutex<Vec<u16>> = Mutex::new(Vec::new());
static PRESSED_BUTTONS: Mutex<u8> = Mutex::new(0);

/// Convenience: run `work` against the injector connection, opening it (with
/// an XTEST requirement) on first use. Failures to open are logged; the
/// caller's action is skipped.
/// Logs injector failures at most once every 10s (a lost X connection would
/// otherwise spam one warning per injected event).
fn warn_injector_unavailable(error: &str) {
    use std::time::Duration;
    static LAST_WARN: Mutex<Option<Instant>> = Mutex::new(None);
    let now = Instant::now();
    let Ok(mut last) = LAST_WARN.lock() else {
        return;
    };
    if last
        .map(|last| now.duration_since(last) < Duration::from_secs(10))
        .unwrap_or(false)
    {
        return;
    }
    *last = Some(now);
    log::warn!("linux input: injector connection unavailable: {error}");
}

fn with_injector<F, T>(work: F) -> Option<T>
where
    F: FnOnce(&X11) -> T,
{
    {
        let mut guard = match INJECTOR.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            match X11::open(true) {
                Ok(x11) => *guard = Some(x11),
                Err(error) => {
                    warn_injector_unavailable(&error);
                    return None;
                }
            }
        }
    }
    let guard = match INJECTOR.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.as_ref().map(work)
}

fn track_key(vk: u16, down: bool) {
    let mut pressed = match PRESSED_KEYS.lock() {
        Ok(pressed) => pressed,
        Err(poisoned) => poisoned.into_inner(),
    };
    if down {
        if !pressed.contains(&vk) {
            pressed.push(vk);
        }
    } else {
        pressed.retain(|code| *code != vk);
    }
}

fn track_button(button: u8, down: bool) {
    let mut pressed = match PRESSED_BUTTONS.lock() {
        Ok(pressed) => pressed,
        Err(poisoned) => poisoned.into_inner(),
    };
    let bit = 1u8 << (button - 1);
    if down {
        *pressed |= bit;
    } else {
        *pressed &= !bit;
    }
}

/// Injects an absolute mouse move in root/physical coordinates.
pub fn inject_mouse_move(x: i32, y: i32, _drag_button: Option<MouseButton>) {
    with_injector(|x11| {
        // X11 has no "drag" event flavour: a held button keeps dragging across
        // synthetic motion on its own, so the drag hint is ignored.
        x11.fake_motion(x, y);
    });
}

pub fn inject_mouse_button(button: MouseButton, down: bool, x: i32, y: i32) {
    let x11_button = x11_button(button);
    with_injector(|x11| {
        // A non-zero position means "move there first" (mirrors the Windows
        // and macOS injectors).
        if x != 0 || y != 0 {
            x11.fake_motion(x, y);
        }
        x11.fake_button(x11_button, down);
    });
    if down {
        track_button(x11_button, true);
    } else {
        track_button(x11_button, false);
    }
}

/// Scroll by wheel notches. X11 has no scroll event: wheel axes are buttons
/// 4/5 (vertical) and 6/7 (horizontal), each notch a press+release pair.
pub fn inject_scroll(delta_x: i32, delta_y: i32) {
    with_injector(|x11| {
        let wheel = |button: u8, clicks: i32| {
            for _ in 0..clicks {
                x11.fake_button(button, true);
                x11.fake_button(button, false);
            }
        };
        if delta_y > 0 {
            wheel(4, delta_y); // up
        } else if delta_y < 0 {
            wheel(5, -delta_y); // down
        }
        if delta_x > 0 {
            wheel(7, delta_x); // right
        } else if delta_x < 0 {
            wheel(6, -delta_x); // left
        }
    });
}

pub fn inject_key(key_code: u16, down: bool) {
    let Some(keysym) = windows_vk_to_keysym(key_code) else {
        log::debug!("linux inject_key: no keysym for vk {key_code:#04x}; dropping");
        return;
    };
    with_injector(|x11| {
        let Some(keycode) = x11.keycode_for_keysym(keysym) else {
            log::debug!(
                "linux inject_key: no keycode for vk {key_code:#04x} (keysym {keysym:#x}); dropping"
            );
            return;
        };
        x11.fake_key(keycode, down);
    });
    if down {
        track_key(key_code, true);
    } else {
        track_key(key_code, false);
    }
}

/// Releases every key and button we injected while it is still held — the
/// remote equivalent of the Windows helper's ReleaseAll — so a session
/// teardown or input source switch can never leave a stuck key or button.
pub fn release_all_injected() {
    let held_keys = match PRESSED_KEYS.lock() {
        Ok(pressed) => pressed.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    // Release in reverse order of pressing so modifier chords unwind cleanly.
    for vk in held_keys.iter().rev() {
        inject_key(*vk, false);
    }
    let held_buttons = {
        let pressed = match PRESSED_BUTTONS.lock() {
            Ok(pressed) => *pressed,
            Err(poisoned) => *poisoned.into_inner(),
        };
        pressed
    };
    for button in 1..=9 {
        if held_buttons & (1u8 << (button - 1)) != 0 {
            with_injector(|x11| x11.fake_button(button, false));
        }
    }
    if let Ok(mut pressed) = PRESSED_KEYS.lock() {
        pressed.clear();
    }
    if let Ok(mut pressed) = PRESSED_BUTTONS.lock() {
        *pressed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_round_trip() {
        for c in b'a'..=b'z' {
            let sym = c as u32;
            assert_eq!(keysym_to_windows_vk(sym), Some((c - 0x20) as u16));
            assert_eq!(keysym_to_windows_vk(sym - 0x20), Some((c - 0x20) as u16));
            assert_eq!(windows_vk_to_keysym((c - 0x20) as u16), Some(sym));
        }
        for c in b'0'..=b'9' {
            let sym = c as u32;
            assert_eq!(keysym_to_windows_vk(sym), Some(c as u16));
            assert_eq!(windows_vk_to_keysym(c as u16), Some(sym));
        }
    }

    #[test]
    fn shifted_punctuation_lands_on_base_vk() {
        // Shift+1 produces '!' — must map to VK 1, never to a shift of its own.
        assert_eq!(keysym_to_windows_vk(0x21), Some(0x31)); // !
        assert_eq!(keysym_to_windows_vk(0x40), Some(0x32)); // @
        assert_eq!(keysym_to_windows_vk(0x3A), Some(0xBA)); // :
        assert_eq!(keysym_to_windows_vk(0x3B), Some(0xBA)); // ;
        assert_eq!(keysym_to_windows_vk(0x7E), Some(0xC0)); // ~
        assert_eq!(keysym_to_windows_vk(0x60), Some(0xC0)); // `
        // And injection of the OEM VK returns the unshifted base keysym.
        assert_eq!(windows_vk_to_keysym(0xBA), Some(0x3B));
        assert_eq!(windows_vk_to_keysym(0xC0), Some(0x60));
        assert_eq!(windows_vk_to_keysym(0xDE), Some(0x27));
        assert_eq!(windows_vk_to_keysym(0xDC), Some(0x5C));
    }

    #[test]
    fn modifiers_keep_left_right_distinction() {
        assert_eq!(keysym_to_windows_vk(ks::SHIFT_L), Some(0xA0));
        assert_eq!(keysym_to_windows_vk(ks::SHIFT_R), Some(0xA1));
        assert_eq!(keysym_to_windows_vk(ks::CONTROL_L), Some(0xA2));
        assert_eq!(keysym_to_windows_vk(ks::CONTROL_R), Some(0xA3));
        assert_eq!(keysym_to_windows_vk(ks::ALT_L), Some(0xA4));
        assert_eq!(keysym_to_windows_vk(ks::ALT_R), Some(0xA5));
        assert_eq!(keysym_to_windows_vk(ks::SUPER_L), Some(0x5B));
        assert_eq!(keysym_to_windows_vk(ks::SUPER_R), Some(0x5C));
        // Generic VKs fall back to the left keysym for injection.
        assert_eq!(windows_vk_to_keysym(0x10), Some(ks::SHIFT_L));
        assert_eq!(windows_vk_to_keysym(0x11), Some(ks::CONTROL_L));
        assert_eq!(windows_vk_to_keysym(0x12), Some(ks::ALT_L));
        assert_eq!(windows_vk_to_keysym(0xA0), Some(ks::SHIFT_L));
        assert_eq!(windows_vk_to_keysym(0xA1), Some(ks::SHIFT_R));
        assert_eq!(windows_vk_to_keysym(0x5B), Some(ks::SUPER_L));
        assert_eq!(windows_vk_to_keysym(0x5C), Some(ks::SUPER_R));
    }

    #[test]
    fn navigation_and_function_keys() {
        assert_eq!(keysym_to_windows_vk(ks::HOME), Some(0x24));
        assert_eq!(keysym_to_windows_vk(ks::END), Some(0x23));
        assert_eq!(keysym_to_windows_vk(ks::PAGE_UP), Some(0x21));
        assert_eq!(keysym_to_windows_vk(ks::DELETE), Some(0x2E));
        assert_eq!(keysym_to_windows_vk(ks::RETURN), Some(0x0D));
        assert_eq!(keysym_to_windows_vk(ks::TAB), Some(0x09));
        for f in 1..=24u16 {
            let sym = 0xFFBE + f as u32 - 1;
            assert_eq!(keysym_to_windows_vk(sym), Some(0x70 + f - 1));
            assert_eq!(windows_vk_to_keysym(0x70 + f - 1), Some(sym));
        }
    }

    #[test]
    fn keypad_maps_both_ways() {
        for i in 0..=9u16 {
            assert_eq!(keysym_to_windows_vk(0xFFB0 + i as u32), Some(0x60 + i));
            assert_eq!(windows_vk_to_keysym(0x60 + i), Some(0xFFB0 + i as u32));
        }
        assert_eq!(keysym_to_windows_vk(ks::KP_DECIMAL), Some(0x6E));
        assert_eq!(keysym_to_windows_vk(ks::KP_DIVIDE), Some(0x6F));
        assert_eq!(keysym_to_windows_vk(ks::KP_ENTER), Some(0x0D));
        // NumLock-off navigation from the keypad.
        assert_eq!(keysym_to_windows_vk(ks::KP_HOME), Some(0x24));
        assert_eq!(keysym_to_windows_vk(ks::KP_LEFT), Some(0x25));
        assert_eq!(keysym_to_windows_vk(ks::KP_BEGIN), Some(0x0C));
        assert_eq!(keysym_to_windows_vk(ks::KP_DELETE), Some(0x2E));
    }

    #[test]
    fn unmapped_keys_return_none() {
        // Media keys have no VK counterpart in the wire protocol.
        assert_eq!(keysym_to_windows_vk(0x1008FF13), None); // XF86AudioRaiseVolume
        assert_eq!(windows_vk_to_keysym(0xAE), None); // VK_VOLUME_MUTE-ish range
        assert_eq!(windows_vk_to_keysym(0xB0), None);
        // Dead keys are dropped on capture (documented limitation).
        assert_eq!(keysym_to_windows_vk(0xFE51), None); // dead_acute
    }

    /// Live-environment sanity check against the current X session: extension
    /// availability, pointer query, keycode<->keysym round trip and a brief
    /// grab+hide cycle. Run with: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires a live X11 session"]
    fn x11_backend_probe() {
        let probe = probe().expect("probe should connect");
        eprintln!(
            "probe: xtest={} xwayland_hint={}",
            probe.xtest_available, probe.xwayland_hint
        );

        let x11 = X11::open(false).expect("open should connect");
        eprintln!("pointer: {:?}", x11.query_pointer());

        for c in ['a', '1', ';', ' ', 'x'] {
            let sym = c as u32;
            let code = x11.keycode_for_keysym(sym);
            let sym_back = code.and_then(|keycode| {
                let sym = x11.effective_keysym(keycode, 0);
                (sym != 0).then_some(sym)
            });
            eprintln!("{c:?}: keycode {code:?}, effective_keysym {sym_back:#x?}");
        }

        // Level-1 resolution with Shift held must yield the shifted keysym.
        let code_a = x11.keycode_for_keysym('a' as u32).expect("an 'a' keycode");
        eprintln!(
            "'a' code {code_a}: level0 {:#x}, shift-level {:#x}",
            x11.effective_keysym(code_a, 0),
            x11.effective_keysym(code_a, 1)
        );

        x11.hide_cursor();
        std::thread::sleep(std::time::Duration::from_millis(50));
        x11.show_cursor();

        eprintln!("grab result: {:?}", x11.grab_pointer_and_keyboard());
        let _ = x11.ungrab_pointer_and_keyboard();
        assert!(x11.query_pointer().is_some(), "pointer should be queryable");

        // XTEST absolute-motion round trip: nudge the real pointer 2px right,
        // verify the server reports the new position, then put it back.
        let (x, y) = x11.query_pointer().expect("pointer before nudge");
        let nudged = (x + 2.0).min(30000.0) as i32;
        x11.fake_motion(nudged, y as i32);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let (x_after, _) = x11.query_pointer().expect("pointer after nudge");
        assert!((x_after - nudged as f64).abs() < 1.0, "xtest motion should move the pointer");
        x11.fake_motion(x as i32, y as i32);
        std::thread::sleep(std::time::Duration::from_millis(30));
        eprintln!("xtest motion round trip ok ({x:.0},{y:.0}) -> {nudged} -> back");
    }
}
