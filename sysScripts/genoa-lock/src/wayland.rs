use std::{fs::File, io::Write, os::fd::AsFd, process::Command};

use crate::{auth, input::KeyboardInput, password::Password};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat,
        wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};

const MAX_OUTPUTS: usize = 16;
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
    surfaces: Vec<LockSurface>,
    password: Password,
    keyboard: KeyboardInput,
}

struct Output {
    global_name: u32,
    output: wl_output::WlOutput,
}

struct LockSurface {
    output_global_name: u32,
    surface: wl_surface::WlSurface,
    session_surface: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    buffer: Option<ShmBuffer>,
    retired_buffers: Vec<ShmBuffer>,
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
    _pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: u32,
    height: u32,
}

impl LockState {
    fn new(options: LockOptions) -> Result<Self, String> {
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
            surfaces: Vec::new(),
            password: Password::default(),
            keyboard: KeyboardInput::new()?,
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

        let lock = lock_manager.lock(queue_handle, ());
        self.lock = Some(lock);
        let outputs = self
            .outputs
            .iter()
            .map(|output| (output.global_name, output.output.clone()))
            .collect::<Vec<_>>();
        for (global_name, output) in outputs {
            self.add_lock_surface(global_name, &output, queue_handle)?;
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

        let session_surface = &self.surfaces[index].session_surface;
        session_surface.ack_configure(serial);

        let needs_buffer = self.surfaces[index]
            .buffer
            .as_ref()
            .is_none_or(|buffer| buffer.width != width || buffer.height != height);
        if needs_buffer {
            if self.surfaces[index].retired_buffers.len() >= MAX_RETIRED_BUFFERS_PER_OUTPUT {
                return Err(
                    "the compositor repeatedly resized a lock surface before releasing old buffers"
                        .to_string(),
                );
            }
            let shm = self
                .shm
                .as_ref()
                .ok_or_else(|| "wl_shm disappeared while locking".to_string())?;
            let new_buffer =
                ShmBuffer::black(shm, queue_handle, output_global_name, width, height)?;
            if let Some(old_buffer) = self.surfaces[index].buffer.replace(new_buffer) {
                // A compositor can still be reading an attached buffer after a
                // resize. Keep its backing file and pool alive until its
                // wl_buffer.release event arrives.
                self.surfaces[index].retired_buffers.push(old_buffer);
            }
        }

        let lock_surface = &self.surfaces[index];
        let buffer = lock_surface
            .buffer
            .as_ref()
            .ok_or_else(|| "could not allocate the lock surface buffer".to_string())?;
        lock_surface.surface.attach(Some(&buffer.buffer), 0, 0);
        lock_surface.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        lock_surface.surface.commit();
        Ok(())
    }

    fn received_key(&mut self, key: u32) {
        if self.lifecycle != LockLifecycle::Locked {
            return;
        }
        if self.keyboard.is_backspace_or_delete(key) {
            self.password.backspace();
        } else if self.keyboard.is_submit(key) {
            self.submit_password();
        } else if let Some(text) = self.keyboard.key_text(key) {
            self.password.push_text(&text);
        }
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
            self.failure = Some("authentication completed without an active lock".to_string());
            self.lifecycle = LockLifecycle::EndedByCompositor;
            return;
        };
        if self.lifecycle != LockLifecycle::Locked {
            self.failure =
                Some("refusing to unlock before the compositor confirmed the lock".to_string());
            self.lifecycle = LockLifecycle::EndedByCompositor;
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
            _pool: pool,
            buffer,
            width,
            height,
        })
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
                "wl_seat" => {
                    registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), queue_handle, ());
                }
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
                            state.failure = Some(error);
                            state.lifecycle = LockLifecycle::EndedByCompositor;
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
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release)
            && let Some(surface) = state
                .surfaces
                .iter_mut()
                .find(|surface| surface.output_global_name == *output_global_name)
        {
            surface
                .retired_buffers
                .retain(|buffer| buffer.buffer != *released_buffer);
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
            state.failure = Some(error);
            state.lifecycle = LockLifecycle::EndedByCompositor;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for LockState {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
            && capabilities.contains(wl_seat::Capability::Keyboard)
        {
            seat.get_keyboard(queue_handle, ());
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for LockState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => match usize::try_from(size) {
                Ok(size) if size <= MAX_KEYMAP_BYTES => {
                    if state.keyboard.load_keymap(fd, size).is_err() {
                        state.failure =
                            Some("the compositor supplied an unusable keyboard layout".to_string());
                        state.lifecycle = LockLifecycle::EndedByCompositor;
                    }
                }
                _ => {
                    state.failure =
                        Some("the compositor supplied an oversized keyboard layout".to_string());
                    state.lifecycle = LockLifecycle::EndedByCompositor;
                }
            },
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => state.received_key(key),
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => state
                .keyboard
                .update_modifiers(mods_depressed, mods_latched, mods_locked, group),
            _ => {}
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
