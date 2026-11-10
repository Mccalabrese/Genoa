use std::{fs::File, io::Write, os::fd::AsFd, process::Command};

use crate::{auth, input::KeyboardInput, password::Password};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat,
        wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use zeroize::Zeroizing;

const MAX_OUTPUTS: usize = 16;
const MAX_SEATS: usize = 16;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const MAX_KEYMAP_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETIRED_BUFFERS_PER_OUTPUT: usize = 4;
const SYSTEMCTL: &str = "/usr/bin/systemctl";

#[derive(Clone, Copy, Default)]
pub struct LockOptions {
    pub suspend_after_lock: bool,
}

pub fn run(options: LockOptions) -> Result<(), String> {
    let connection = Connection::connect_to_env()
        .map_err(|error| format!("could not connect to the Wayland session: {error}"))?;
    let mut event_queue = connection.new_event_queue();
    let queue_handle = event_queue.handle();
    connection.display().get_registry(&queue_handle, ());

    let mut state = LockState::new(options)?;
    event_queue
        .roundtrip(&mut state)
        .map_err(|error| format!("could not discover Wayland globals: {error}"))?;
    if let Some(error) = state.failure.take() {
        return Err(error);
    }
    state.start_lock(&queue_handle)?;

    while state.should_run() {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|error| format!("Wayland connection failed: {error}"))?;
        if let Some(error) = state.failure.take() {
            return Err(error);
        }
        if state.lifecycle == LockLifecycle::UnlockAwaitingSync {
            // The protocol requires a sync after `unlock_and_destroy`: exiting
            // before this callback would make successful unlock delivery
            // dependent on connection teardown timing.
            connection.display().sync(&queue_handle, ());
            state.lifecycle = LockLifecycle::UnlockSyncPending;
        }
    }

    if state.lifecycle == LockLifecycle::Unlocked {
        Ok(())
    } else {
        Err("the compositor ended the lock session".to_string())
    }
}

struct LockState {
    options: LockOptions,
    lifecycle: LockLifecycle,
    suspend_requested: bool,
    failure: Option<String>,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    lock_manager: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    lock: Option<ext_session_lock_v1::ExtSessionLockV1>,
    outputs: Vec<Output>,
    seats: Vec<Seat>,
    surfaces: Vec<LockSurface>,
    password: Password,
}

struct Output {
    global_name: u32,
    output: wl_output::WlOutput,
}

struct Seat {
    global_name: u32,
    seat: wl_seat::WlSeat,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    input: KeyboardInput,
}

enum KeyAction {
    Ignore,
    Backspace,
    Submit,
    Text(Zeroizing<String>),
}

impl Seat {
    fn translate_key(&self, key: u32) -> KeyAction {
        if self.input.is_backspace_or_delete(key) {
            KeyAction::Backspace
        } else if self.input.is_submit(key) {
            KeyAction::Submit
        } else {
            self.input
                .key_text(key)
                .map_or(KeyAction::Ignore, KeyAction::Text)
        }
    }
}

struct LockSurface {
    output_global_name: u32,
    surface: wl_surface::WlSurface,
    session_surface: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    buffer: Option<ShmBuffer>,
    retired_buffers: Vec<ShmBuffer>,
    pending_configure: Option<PendingConfigure>,
}

#[derive(Clone, Copy)]
struct PendingConfigure {
    serial: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum LockLifecycle {
    Requested,
    Locked,
    UnlockAwaitingSync,
    UnlockSyncPending,
    Unlocked,
    EndedByCompositor,
}

struct ShmBuffer {
    // The backing objects must outlive both the Wayland buffer and every use of
    // that buffer by the compositor.
    _file: File,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: u32,
    height: u32,
    released: bool,
}

impl LockState {
    fn new(options: LockOptions) -> Result<Self, String> {
        // Fail before sending a lock request if libxkbcommon is unavailable.
        // Each seat owns a separate input context, so this temporary probe is
        // deliberately dropped immediately.
        drop(KeyboardInput::new()?);
        Ok(Self {
            options,
            lifecycle: LockLifecycle::Requested,
            suspend_requested: false,
            failure: None,
            compositor: None,
            shm: None,
            lock_manager: None,
            lock: None,
            outputs: Vec::new(),
            seats: Vec::new(),
            surfaces: Vec::new(),
            password: Password::default(),
        })
    }

    fn start_lock(&mut self, queue_handle: &QueueHandle<Self>) -> Result<(), String> {
        if self.compositor.is_none() {
            return Err("the compositor does not provide wl_compositor".to_string());
        }
        let lock_manager = self.lock_manager.as_ref().ok_or_else(|| {
            "the compositor does not support ext-session-lock-v1; refusing an insecure fallback"
                .to_string()
        })?;

        if self.shm.is_none() {
            return Err("the compositor does not provide wl_shm".to_string());
        }
        if self.outputs.is_empty() {
            return Err("the compositor did not report an output".to_string());
        }
        if self.seats.is_empty() {
            return Err("the compositor did not report a wl_seat".to_string());
        }

        let lock = lock_manager.lock(queue_handle, ());
        self.lock = Some(lock);
        let outputs = self
            .outputs
            .iter()
            .map(|output| (output.global_name, output.output.clone()))
            .collect::<Vec<_>>();
        for (global_name, output) in outputs {
            if let Err(error) = self.add_lock_surface(global_name, &output, queue_handle) {
                // Once the lock request has been sent, returning from this
                // function could disconnect after the compositor has locked
                // but before its `locked` event reaches us. Keep the client
                // alive and let the compositor's secure fallback cover the
                // affected output instead.
                Self::report_runtime_error(error);
            }
        }
        Ok(())
    }

    fn add_lock_surface(
        &mut self,
        output_global_name: u32,
        output: &wl_output::WlOutput,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<(), String> {
        if self
            .surfaces
            .iter()
            .any(|surface| surface.output_global_name == output_global_name)
        {
            return Ok(());
        }
        let compositor = self
            .compositor
            .as_ref()
            .ok_or_else(|| "wl_compositor disappeared while locking".to_string())?;
        let lock = self
            .lock
            .as_ref()
            .ok_or_else(|| "session lock disappeared while adding an output".to_string())?;
        let surface = compositor.create_surface(queue_handle, ());
        let session_surface =
            lock.get_lock_surface(&surface, output, queue_handle, output_global_name);
        self.surfaces.push(LockSurface {
            output_global_name,
            surface,
            session_surface,
            buffer: None,
            retired_buffers: Vec::new(),
            pending_configure: None,
        });
        Ok(())
    }

    const fn should_run(&self) -> bool {
        matches!(
            self.lifecycle,
            LockLifecycle::Requested
                | LockLifecycle::Locked
                | LockLifecycle::UnlockAwaitingSync
                | LockLifecycle::UnlockSyncPending
        )
    }

    fn configure_surface(
        &mut self,
        output_global_name: u32,
        serial: u32,
        width: u32,
        height: u32,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<(), String> {
        let index = self
            .surfaces
            .iter()
            .position(|surface| surface.output_global_name == output_global_name)
            .ok_or_else(|| "received a configure event for an unknown output".to_string())?;

        self.surfaces[index].pending_configure = Some(PendingConfigure {
            serial,
            width,
            height,
        });
        self.render_pending_configure(index, queue_handle)
    }

    fn render_pending_configure(
        &mut self,
        index: usize,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<(), String> {
        let Some(requested) = self.surfaces[index].pending_configure else {
            return Ok(());
        };
        let output_global_name = self.surfaces[index].output_global_name;
        let needs_buffer = self.surfaces[index].buffer.as_ref().is_none_or(|buffer| {
            buffer.width != requested.width || buffer.height != requested.height
        });

        if needs_buffer {
            if self.surfaces[index].retired_buffers.len() >= MAX_RETIRED_BUFFERS_PER_OUTPUT {
                return Err(
                    "waiting for the compositor to release an older lock-surface buffer"
                        .to_string(),
                );
            }
            let shm = self
                .shm
                .as_ref()
                .ok_or_else(|| "wl_shm disappeared while locking".to_string())?;
            let new_buffer = ShmBuffer::black(
                shm,
                queue_handle,
                output_global_name,
                requested.width,
                requested.height,
            )?;
            if let Some(old_buffer) = self.surfaces[index].buffer.replace(new_buffer) {
                // A compositor can still be reading an attached buffer after a
                // resize. Keep its backing file and pool alive until its
                // wl_buffer.release event arrives.
                if !old_buffer.released {
                    self.surfaces[index].retired_buffers.push(old_buffer);
                }
            }
        }

        let lock_surface = &mut self.surfaces[index];
        let buffer = lock_surface
            .buffer
            .as_mut()
            .ok_or_else(|| "could not allocate the lock surface buffer".to_string())?;
        buffer.released = false;
        lock_surface.session_surface.ack_configure(requested.serial);
        lock_surface.surface.attach(Some(&buffer.buffer), 0, 0);
        lock_surface.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        lock_surface.surface.commit();
        lock_surface.pending_configure = None;
        Ok(())
    }

    fn report_runtime_error(error: impl AsRef<str>) {
        eprintln!(
            "genoa-lock: {0}; keeping the session locked",
            error.as_ref()
        );
    }

    fn submit_password(&mut self) {
        let password = self.password.take();
        if password.is_empty() {
            return;
        }

        // Authentication must fail closed. Do not expose PAM details on the
        // lock screen or terminal because they can disclose policy.
        if auth::authenticate_current_user(&password) == Ok(true) {
            self.unlock();
        }
    }

    fn unlock(&mut self) {
        let Some(lock) = self.lock.as_ref() else {
            Self::report_runtime_error("authentication completed without an active lock");
            return;
        };
        if self.lifecycle != LockLifecycle::Locked {
            Self::report_runtime_error(
                "refusing to unlock before the compositor confirmed the lock",
            );
            return;
        }

        lock.unlock_and_destroy();
        self.lifecycle = LockLifecycle::UnlockAwaitingSync;
    }

    fn suspend_after_lock(&mut self) {
        if !self.options.suspend_after_lock || self.suspend_requested {
            return;
        }
        self.suspend_requested = true;
        let _ = Command::new(SYSTEMCTL).arg("suspend").status();
    }
}

impl ShmBuffer {
    fn black(
        shm: &wl_shm::WlShm,
        queue_handle: &QueueHandle<LockState>,
        output_global_name: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let byte_len = checked_buffer_len(width, height)?;
        let byte_len_i32 = i32::try_from(byte_len)
            .map_err(|_| "lock surface buffer is too large for Wayland".to_string())?;
        let stride = width
            .checked_mul(4)
            .and_then(|stride| i32::try_from(stride).ok())
            .ok_or_else(|| "lock surface stride is too large".to_string())?;

        let mut file = tempfile::tempfile()
            .map_err(|error| format!("could not create a private graphics buffer: {error}"))?;
        file.set_len(u64::try_from(byte_len).map_err(|_| "lock buffer is too large".to_string())?)
            .map_err(|error| format!("could not size the graphics buffer: {error}"))?;
        let mut black_pixels = [0_u8; 4096];
        for alpha in black_pixels.iter_mut().skip(3).step_by(4) {
            *alpha = 0xff;
        }
        let mut remaining = byte_len;
        while remaining > 0 {
            let chunk_len = remaining.min(black_pixels.len());
            file.write_all(&black_pixels[..chunk_len])
                .map_err(|error| format!("could not initialise the graphics buffer: {error}"))?;
            remaining -= chunk_len;
        }

        let pool = shm.create_pool(file.as_fd(), byte_len_i32, queue_handle, ());
        let buffer = pool.create_buffer(
            0,
            i32::try_from(width).map_err(|_| "lock surface width is too large".to_string())?,
            i32::try_from(height).map_err(|_| "lock surface height is too large".to_string())?,
            stride,
            wl_shm::Format::Argb8888,
            queue_handle,
            output_global_name,
        );

        Ok(Self {
            _file: file,
            pool,
            buffer,
            width,
            height,
            released: false,
        })
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

fn checked_buffer_len(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("the compositor requested a zero-sized lock surface".to_string());
    }
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| "lock surface dimensions overflow".to_string())?;
    let byte_len = pixels
        .checked_mul(4)
        .filter(|size| *size <= MAX_BUFFER_BYTES)
        .ok_or_else(|| "lock surface exceeds the 64 MiB safety limit".to_string())?;
    Ok(byte_len)
}

fn release_keyboard(keyboard: wl_keyboard::WlKeyboard) {
    if keyboard.version() >= 3 {
        keyboard.release();
    }
}

fn release_seat(seat: wl_seat::WlSeat) {
    if seat.version() >= 5 {
        seat.release();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for LockState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_compositor" if state.compositor.is_none() => {
                    state.compositor = Some(registry.bind(name, version.min(4), queue_handle, ()));
                }
                "wl_shm" if state.shm.is_none() => {
                    state.shm = Some(registry.bind(name, version.min(1), queue_handle, ()));
                }
                "wl_seat" if state.seats.len() < MAX_SEATS => match KeyboardInput::new() {
                    Ok(input) => {
                        let seat = registry.bind::<wl_seat::WlSeat, _, _>(
                            name,
                            version.min(7),
                            queue_handle,
                            (),
                        );
                        state.seats.push(Seat {
                            global_name: name,
                            seat,
                            keyboard: None,
                            input,
                        });
                        if state.lock.is_none() {
                            state.failure = None;
                        }
                    }
                    Err(error) => {
                        if state.lock.is_some() || !state.seats.is_empty() {
                            Self::report_runtime_error(format!(
                                "could not initialize input for seat {name}: {error}"
                            ));
                        } else {
                            state.failure = Some(error);
                        }
                    }
                },
                "wl_seat" => eprintln!(
                    "genoa-lock: ignoring seat {name}; reached the {MAX_SEATS}-seat limit"
                ),
                "wl_output" if state.outputs.len() < MAX_OUTPUTS => {
                    let output = registry.bind::<wl_output::WlOutput, _, _>(
                        name,
                        version.min(4),
                        queue_handle,
                        (),
                    );
                    state.outputs.push(Output {
                        global_name: name,
                        output,
                    });
                    if state.lock.is_some() {
                        let output = state.outputs.last().map(|item| item.output.clone());
                        if let Some(output) = output
                            && let Err(error) = state.add_lock_surface(name, &output, queue_handle)
                        {
                            Self::report_runtime_error(error);
                        }
                    }
                }
                "ext_session_lock_manager_v1" if state.lock_manager.is_none() => {
                    state.lock_manager =
                        Some(registry.bind(name, version.min(1), queue_handle, ()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(index) = state.seats.iter().position(|seat| seat.global_name == name) {
                    let mut removed = state.seats.swap_remove(index);
                    if let Some(keyboard) = removed.keyboard.take() {
                        release_keyboard(keyboard);
                    }
                    release_seat(removed.seat);
                }
                state.outputs.retain(|output| output.global_name != name);
                if let Some(index) = state
                    .surfaces
                    .iter()
                    .position(|surface| surface.output_global_name == name)
                {
                    let removed = state.surfaces.swap_remove(index);
                    removed.session_surface.destroy();
                    removed.surface.destroy();
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(LockState: ignore wl_compositor::WlCompositor);
delegate_noop!(LockState: ignore wl_surface::WlSurface);
delegate_noop!(LockState: ignore wl_shm::WlShm);
delegate_noop!(LockState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(LockState: ignore wl_output::WlOutput);
delegate_noop!(LockState: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1);

impl Dispatch<wl_buffer::WlBuffer, u32> for LockState {
    fn event(
        state: &mut Self,
        released_buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        output_global_name: &u32,
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release)
            && let Some(index) = state
                .surfaces
                .iter()
                .position(|surface| surface.output_global_name == *output_global_name)
        {
            {
                let surface = &mut state.surfaces[index];
                if let Some(active) = surface.buffer.as_mut()
                    && active.buffer == *released_buffer
                {
                    active.released = true;
                }
                surface
                    .retired_buffers
                    .retain(|buffer| buffer.buffer != *released_buffer);
            }
            if state.surfaces[index].pending_configure.is_some()
                && let Err(error) = state.render_pending_configure(index, queue_handle)
            {
                Self::report_runtime_error(error);
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for LockState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. })
            && state.lifecycle == LockLifecycle::UnlockSyncPending
        {
            state.lifecycle = LockLifecycle::Unlocked;
        }
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for LockState {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => {
                state.lifecycle = LockLifecycle::Locked;
                state.suspend_after_lock();
            }
            ext_session_lock_v1::Event::Finished => {
                state.lifecycle = LockLifecycle::EndedByCompositor;
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, u32> for LockState {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        output_global_name: &u32,
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
            && let Err(error) =
                state.configure_surface(*output_global_name, serial, width, height, queue_handle)
        {
            Self::report_runtime_error(error);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for LockState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
            && let Some(known_seat) = state.seats.iter_mut().find(|known| known.seat == *seat)
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                if known_seat.keyboard.is_none() {
                    known_seat.keyboard =
                        Some(seat.get_keyboard(queue_handle, known_seat.global_name));
                }
            } else if let Some(keyboard) = known_seat.keyboard.take() {
                release_keyboard(keyboard);
                known_seat.input.reset();
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, u32> for LockState {
    fn event(
        state: &mut Self,
        keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        seat_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let lock_confirmed = state.lifecycle == LockLifecycle::Locked;
        let action = {
            let Some(seat) = state.seats.iter_mut().find(|seat| {
                seat.global_name == *seat_name
                    && seat
                        .keyboard
                        .as_ref()
                        .is_some_and(|current| current == keyboard)
            }) else {
                return;
            };

            match event {
                wl_keyboard::Event::Keymap {
                    format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                    fd,
                    size,
                } => {
                    match usize::try_from(size) {
                        Ok(size) if size <= MAX_KEYMAP_BYTES => {
                            if let Err(error) = seat.input.load_keymap(fd, size) {
                                // `load_keymap` changes its XKB state only after
                                // every validation and allocation succeeds. A bad
                                // replacement therefore leaves the last usable
                                // layout available for password entry.
                                Self::report_runtime_error(error);
                            }
                        }
                        _ => Self::report_runtime_error(
                            "the compositor supplied an oversized keyboard layout",
                        ),
                    }
                    None
                }
                wl_keyboard::Event::Keymap { .. } => {
                    // The locker cannot safely translate raw keycodes without
                    // XKB. Do not keep a layout from an earlier keymap.
                    seat.input.reset();
                    None
                }
                wl_keyboard::Event::Key {
                    key,
                    state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                    ..
                } if lock_confirmed => Some(seat.translate_key(key)),
                wl_keyboard::Event::Modifiers {
                    mods_depressed,
                    mods_latched,
                    mods_locked,
                    group,
                    ..
                } => {
                    seat.input
                        .update_modifiers(mods_depressed, mods_latched, mods_locked, group);
                    None
                }
                _ => None,
            }
        };

        match action {
            Some(KeyAction::Backspace) => state.password.backspace(),
            Some(KeyAction::Submit) => state.submit_password(),
            Some(KeyAction::Text(text)) => state.password.push_text(&text),
            Some(KeyAction::Ignore) | None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::checked_buffer_len;

    #[test]
    fn rejects_zero_dimension_buffers() {
        assert!(checked_buffer_len(0, 1080).is_err());
        assert!(checked_buffer_len(1920, 0).is_err());
    }

    #[test]
    fn accepts_a_4k_buffer() {
        assert_eq!(checked_buffer_len(3840, 2160), Ok(33_177_600));
    }

    #[test]
    fn caps_unreasonable_buffers() {
        assert!(checked_buffer_len(16_384, 16_384).is_err());
    }
}
