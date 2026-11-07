//! The narrow, owned wrapper around libxkbcommon used by the Wayland keyboard.
//!
//! `xkbcommon-dl` provides pre-generated dynamic bindings, avoiding a bindgen
//! and libclang build dependency in the installed locker. This module owns all
//! foreign pointers, validates Wayland's supplied keymap size before reading,
//! and exposes only Rust values to the rest of the program.

#![allow(unsafe_code)]

use std::{fs::File, io::Read, os::fd::OwnedFd, ptr::NonNull};

use xkbcommon_dl::{
    XkbCommon, keysyms, xkb_context, xkb_context_flags, xkb_keymap, xkb_keymap_compile_flags,
    xkb_keymap_format, xkb_state,
};
use zeroize::Zeroizing;

const UTF8_BUFFER_LEN: usize = 256;

pub struct KeyboardInput {
    library: &'static XkbCommon,
    context: NonNull<xkb_context>,
    state: Option<KeyboardState>,
}

struct KeyboardState {
    keymap: NonNull<xkb_keymap>,
    state: NonNull<xkb_state>,
    // Keep the source alive for the full keymap lifetime. libxkbcommon parses
    // the buffer today, but retaining it makes that lifetime explicit.
    _source: String,
}

impl KeyboardInput {
    pub fn new() -> Result<Self, String> {
        let library = xkbcommon_dl::xkbcommon_option()
            .ok_or_else(|| "libxkbcommon is unavailable".to_string())?;
        // SAFETY: this passes the documented flags value and does not retain
        // borrowed Rust data. The returned pointer is immediately null-checked
        // and owned by `KeyboardInput` until its matching unref call.
        let context = NonNull::new(unsafe {
            (library.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS)
        })
        .ok_or_else(|| "could not create an XKB context".to_string())?;
        Ok(Self {
            library,
            context,
            state: None,
        })
    }

    pub fn load_keymap(&mut self, fd: OwnedFd, size: usize) -> Result<(), String> {
        let mut bytes = Vec::with_capacity(size);
        let mut file = File::from(fd).take(u64::try_from(size).map_err(|_| "keymap is too large")?);
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("could not read the keyboard layout: {error}"))?;
        if bytes.len() != size {
            return Err("the compositor supplied a truncated keyboard layout".to_string());
        }
        let source = String::from_utf8(bytes)
            .map_err(|_| "the compositor supplied a non-UTF-8 keyboard layout".to_string())?;
        // SAFETY: context is live for this call; `source` is valid UTF-8 and
        // retained in `KeyboardState` for at least as long as the keymap; its
        // pointer/length describe exactly that allocation.
        let keymap = NonNull::new(unsafe {
            (self.library.xkb_keymap_new_from_buffer)(
                self.context.as_ptr(),
                source.as_ptr().cast(),
                source.len(),
                xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            )
        })
        .ok_or_else(|| "libxkbcommon rejected the keyboard layout".to_string())?;
        // SAFETY: `keymap` is non-null and owned until after `state` is freed.
        let state = NonNull::new(unsafe { (self.library.xkb_state_new)(keymap.as_ptr()) })
            .ok_or_else(|| "could not create keyboard state".to_string())?;

        self.state = Some(KeyboardState {
            keymap,
            state,
            _source: source,
        });
        Ok(())
    }

    pub fn key_text(&self, wayland_key: u32) -> Option<Zeroizing<String>> {
        let state = self.state.as_ref()?;
        let keycode = wayland_key.checked_add(8)?;
        let mut buffer = Zeroizing::new([0_i8; UTF8_BUFFER_LEN]);
        // SAFETY: state is live, `buffer` is writable for its exact supplied
        // length, and libxkbcommon writes at most that many bytes.
        let written = unsafe {
            (self.library.xkb_state_key_get_utf8)(
                state.state.as_ptr(),
                keycode,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        let written = usize::try_from(written).ok()?;
        if written >= buffer.len() {
            return None;
        }
        let bytes = buffer[..written]
            .iter()
            .map(|byte| byte.cast_unsigned())
            .collect::<Vec<_>>();
        String::from_utf8(bytes).ok().map(Zeroizing::new)
    }

    pub fn keysym(&self, wayland_key: u32) -> Option<u32> {
        let state = self.state.as_ref()?;
        let keycode = wayland_key.checked_add(8)?;
        // SAFETY: state is live and `keycode` is the protocol key plus XKB's
        // documented offset.
        Some(unsafe { (self.library.xkb_state_key_get_one_sym)(state.state.as_ptr(), keycode) })
    }

    pub fn update_modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        if let Some(state) = self.state.as_ref() {
            // SAFETY: state is live and all masks/layout indexes are received
            // directly from the active Wayland seat.
            let _ = unsafe {
                (self.library.xkb_state_update_mask)(
                    state.state.as_ptr(),
                    depressed,
                    latched,
                    locked,
                    0,
                    0,
                    group,
                )
            };
        }
    }

    pub fn is_backspace_or_delete(&self, wayland_key: u32) -> bool {
        matches!(
            self.keysym(wayland_key),
            Some(keysyms::BackSpace | keysyms::Delete)
        )
    }

    pub fn is_submit(&self, wayland_key: u32) -> bool {
        matches!(
            self.keysym(wayland_key),
            Some(keysyms::Return | keysyms::KP_Enter)
        )
    }
}

impl Drop for KeyboardState {
    fn drop(&mut self) {
        // Both pointers originate from libxkbcommon constructors and are owned
        // exclusively by this value. State must be released before keymap.
        let library = xkbcommon_dl::xkbcommon_handle();
        // SAFETY: both pointers are non-null, each is owned exactly once here,
        // and the state is released before its keymap.
        unsafe {
            (library.xkb_state_unref)(self.state.as_ptr());
            (library.xkb_keymap_unref)(self.keymap.as_ptr());
        }
    }
}

impl Drop for KeyboardInput {
    fn drop(&mut self) {
        // Struct fields are dropped after `Drop::drop` returns. Release the
        // state/keymap first because both are children of this XKB context.
        drop(self.state.take());
        // SAFETY: `context` is non-null and owned exactly once by this value.
        unsafe { (self.library.xkb_context_unref)(self.context.as_ptr()) };
    }
}
