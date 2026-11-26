use std::{
    fs::File,
    io::{ErrorKind, Seek, SeekFrom, Write},
    os::fd::{AsFd, OwnedFd},
    process::Command,
    sync::{Arc, mpsc::Receiver},
    time::{Duration, Instant},
};

use crate::{
    auth, input::KeyboardInput, instance::InstanceGuard, password::Password, visual::Appearance,
};
use rustix::{
    event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll},
    io::{self, retry_on_intr},
};
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
const MAX_GRAPHICS_BYTES: usize = 256 * 1024 * 1024;
const MAX_KEYMAP_BYTES: usize = 4 * 1024 * 1024;
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const VISUAL_REFRESH_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Default)]
pub struct LockOptions {
    pub suspend_after_lock: bool,
}

pub fn run(options: LockOptions) -> Result<(), String> {
    // This must outlive the Wayland connection. A second launcher should fail
    // before it can create another session-lock client or receive keyboard
    // focus intended for the active locker.
    let _instance_guard = InstanceGuard::acquire()?;
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
            .dispatch_pending(&mut state)
            .map_err(|error| format!("Wayland connection failed: {error}"))?;
        if let Some(error) = state.failure.take() {
            return Err(error);
        }
        state.redraw_if_due(&queue_handle);
        state.start_unlock_sync(&connection, &queue_handle);
        if !state.should_run() {
            continue;
        }

        let needs_flush = match event_queue.flush() {
            Ok(()) => false,
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == ErrorKind::WouldBlock =>
            {
                true
            }
            Err(error) => return Err(format!("Wayland connection failed: {error}")),
        };
        let Some(read_guard) = event_queue.prepare_read() else {
            continue;
        };
        let (wayland_ready, authentication_ready) = {
            let mut wayland_events = PollFlags::IN;
            if needs_flush {
                wayland_events |= PollFlags::OUT;
            }
            let mut fds = [
                PollFd::from_borrowed_fd(read_guard.connection_fd(), wayland_events),
                PollFd::new(state.authentication_event.as_ref(), PollFlags::IN),
            ];
            let timeout = state.visual_poll_timeout();
            retry_on_intr(|| poll(&mut fds, timeout.as_ref()))
                .map_err(|error| format!("could not wait for lock events: {error}"))?;
            let wayland_ready = fds[0]
                .revents()
                .intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP);
            (wayland_ready, fds[1].revents().contains(PollFlags::IN))
        };

        if wayland_ready {
            match read_guard.read() {
                Ok(_) => {
                    event_queue
                        .dispatch_pending(&mut state)
                        .map_err(|error| format!("Wayland connection failed: {error}"))?;
                    if let Some(error) = state.failure.take() {
                        return Err(error);
                    }
                }
                Err(wayland_client::backend::WaylandError::Io(error))
                    if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("Wayland connection failed: {error}")),
            }
        } else {
            drop(read_guard);
        }
        if authentication_ready {
            state.finish_authentication();
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
    appearance: Appearance,
    visual_redraw_after: Option<Instant>,
    password: Password,
    authentication_event: Arc<OwnedFd>,
    authentication: Option<AuthenticationAttempt>,
}

struct AuthenticationAttempt {
    receiver: Receiver<auth::AuthenticationResult>,
}

struct Output {
    global_name: u32,
    output: wl_output::WlOutput,
}

struct Seat {
    global_name: u32,
    proxy: wl_seat::WlSeat,
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
    spare_buffer: Option<ShmBuffer>,
    retired_buffer: Option<ShmBuffer>,
    pending_configure: Option<PendingConfigure>,
    redraw_pending: bool,
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
    file: File,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: u32,
    height: u32,
    byte_len: usize,
    released: bool,
}

impl LockState {
    fn new(options: LockOptions) -> Result<Self, String> {
        // Fail before sending a lock request if libxkbcommon is unavailable.
        // Each seat owns a separate input context, so this temporary probe is
        // deliberately dropped immediately.
        drop(KeyboardInput::new()?);
        let authentication_event = eventfd(0, EventfdFlags::CLOEXEC)
            .map_err(|error| format!("could not create the authentication notifier: {error}"))?;
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
            appearance: Appearance::load()?,
            visual_redraw_after: None,
            password: Password::default(),
            authentication_event: Arc::new(authentication_event),
            authentication: None,
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
            spare_buffer: None,
            retired_buffer: None,
            pending_configure: None,
            redraw_pending: false,
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
        let needs_new_buffer = self.surfaces[index].buffer.as_ref().is_none_or(|buffer| {
            buffer.width != requested.width || buffer.height != requested.height
        });
        if self.surfaces[index]
            .buffer
            .as_ref()
            .is_some_and(|buffer| !buffer.released)
            || self.surfaces[index].retired_buffer.is_some()
        {
            // A Wayland buffer is immutable while the compositor is reading
            // it. Retain the latest configure and finish it at release.
            return Ok(());
        }

        if needs_new_buffer {
            let new_buffer =
                self.new_surface_buffer(index, requested.width, requested.height, queue_handle)?;
            self.surfaces[index].buffer = Some(new_buffer);
            // A released buffer is safe to drop. Old-size spares cannot be
            // reused after a configure, so discard them with the old frame.
            self.surfaces[index].spare_buffer = None;
        }
        self.paint_and_commit(index, Some(requested.serial))?;
        self.surfaces[index].pending_configure = None;
        Ok(())
    }

    fn report_runtime_error(error: impl AsRef<str>) {
        eprintln!(
            "genoa-lock: {0}; keeping the session locked",
            error.as_ref()
        );
    }

    // Presentation is deliberately decoupled from input and authentication.
    // Key events only record a future repaint; the main loop coalesces rapid
    // typing and renders outside every Wayland event callback.
    fn request_visual_redraw(&mut self) {
        if self.visual_redraw_after.is_none() {
            self.visual_redraw_after = Some(Instant::now() + VISUAL_REFRESH_INTERVAL);
        }
    }

    fn request_visual_redraw_now(&mut self) {
        self.visual_redraw_after = Some(Instant::now());
    }

    fn redraw_if_due(&mut self, queue_handle: &QueueHandle<Self>) {
        if self
            .visual_redraw_after
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.visual_redraw_after = None;
            for index in 0..self.surfaces.len() {
                if let Err(error) = self.redraw_surface(index, queue_handle) {
                    Self::report_runtime_error(error);
                }
            }
        }
    }

    fn visual_poll_timeout(&self) -> Option<Timespec> {
        let deadline = self.visual_redraw_after?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        Some(Timespec {
            tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(remaining.subsec_nanos()),
        })
    }

    fn redraw_surface(
        &mut self,
        index: usize,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<(), String> {
        if self.surfaces[index].buffer.is_none() {
            return Err("cannot redraw an unconfigured lock surface".to_string());
        }
        if self.surfaces[index]
            .buffer
            .as_ref()
            .is_some_and(|buffer| buffer.released)
        {
            return self.paint_and_commit(index, None);
        }

        if let Some(spare) = self.surfaces[index].spare_buffer.take() {
            if !spare.released {
                return Err("a reusable lock buffer was not released".to_string());
            }
            let active = self.surfaces[index]
                .buffer
                .replace(spare)
                .ok_or_else(|| "could not replace the active lock buffer".to_string())?;
            self.surfaces[index].retired_buffer = Some(active);
            return self.paint_and_commit(index, None);
        }

        if self.surfaces[index].retired_buffer.is_some() {
            self.surfaces[index].redraw_pending = true;
            return Ok(());
        }

        let (width, height) = self.surfaces[index]
            .buffer
            .as_ref()
            .map(|buffer| (buffer.width, buffer.height))
            .ok_or_else(|| "cannot size an unconfigured lock surface".to_string())?;
        let replacement = self.new_surface_buffer(index, width, height, queue_handle)?;
        let active = self.surfaces[index]
            .buffer
            .replace(replacement)
            .ok_or_else(|| "could not replace the active lock buffer".to_string())?;
        self.surfaces[index].retired_buffer = Some(active);
        self.paint_and_commit(index, None)
    }

    fn new_surface_buffer(
        &self,
        index: usize,
        width: u32,
        height: u32,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<ShmBuffer, String> {
        let byte_len = checked_buffer_len(width, height)?;
        // A new shared-memory buffer and a temporary rendered frame coexist
        // until the frame is copied into that buffer.
        if !graphics_budget_permits(self.graphics_bytes(), byte_len.saturating_mul(2)) {
            return Err("lock graphics memory limit would be exceeded".to_string());
        }

        let shm = self
            .shm
            .as_ref()
            .ok_or_else(|| "wl_shm disappeared while allocating a lock buffer".to_string())?;
        let output_global_name = self.surfaces[index].output_global_name;
        ShmBuffer::new(
            shm,
            queue_handle,
            output_global_name,
            width,
            height,
            byte_len,
        )
    }

    fn paint_and_commit(
        &mut self,
        index: usize,
        configure_serial: Option<u32>,
    ) -> Result<(), String> {
        let (width, height) = self.surfaces[index]
            .buffer
            .as_ref()
            .map(|buffer| (buffer.width, buffer.height))
            .ok_or_else(|| "cannot paint an unconfigured lock surface".to_string())?;
        let frame = self.appearance.frame(
            width,
            height,
            self.password.display_len(),
            self.authentication.is_some(),
        )?;

        let lock_surface = &mut self.surfaces[index];
        let buffer = lock_surface
            .buffer
            .as_mut()
            .ok_or_else(|| "could not access the lock surface buffer".to_string())?;
        buffer.write_frame(&frame)?;
        buffer.released = false;
        lock_surface.redraw_pending = false;
        if let Some(serial) = configure_serial {
            lock_surface.session_surface.ack_configure(serial);
        }
        lock_surface.surface.attach(Some(&buffer.buffer), 0, 0);
        lock_surface.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        lock_surface.surface.commit();
        Ok(())
    }

    fn graphics_bytes(&self) -> usize {
        self.surfaces
            .iter()
            .flat_map(|surface| {
                [
                    surface.buffer.as_ref(),
                    surface.spare_buffer.as_ref(),
                    surface.retired_buffer.as_ref(),
                ]
            })
            .flatten()
            .fold(0_usize, |total, buffer| {
                total.saturating_add(buffer.byte_len)
            })
    }

    fn submit_password(&mut self) {
        if self.authentication.is_some() {
            return;
        }
        let password = self.password.take();
        if password.is_empty() {
            return;
        }

        match auth::start_authentication(password, self.authentication_event.clone()) {
            Ok(receiver) => {
                self.authentication = Some(AuthenticationAttempt { receiver });
                self.request_visual_redraw();
            }
            Err(error) => Self::report_runtime_error(error),
        }
    }

    fn finish_authentication(&mut self) {
        let mut counter = [0_u8; 8];
        let read = retry_on_intr(|| io::read(self.authentication_event.as_ref(), &mut counter));
        if !matches!(read, Ok(8)) {
            self.authentication = None;
            Self::report_runtime_error("could not receive the authentication result");
            self.request_visual_redraw();
            return;
        }
        let Some(attempt) = self.authentication.as_ref() else {
            Self::report_runtime_error(
                "received an authentication result without an active attempt",
            );
            return;
        };
        let authenticated = match attempt.receiver.try_recv() {
            Ok(Ok(authenticated)) => authenticated,
            Ok(Err(error)) => {
                self.authentication = None;
                Self::report_runtime_error(format!("authentication is unavailable: {error}"));
                self.request_visual_redraw();
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Self::report_runtime_error("the authentication worker ended unexpectedly");
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                Self::report_runtime_error("the authentication worker signalled without a result");
                return;
            }
        };
        self.authentication = None;
        if authenticated {
            self.unlock();
        } else {
            self.request_visual_redraw();
        }
    }

    fn start_unlock_sync(&mut self, connection: &Connection, queue_handle: &QueueHandle<Self>) {
        if self.lifecycle == LockLifecycle::UnlockAwaitingSync {
            connection.display().sync(queue_handle, ());
            self.lifecycle = LockLifecycle::UnlockSyncPending;
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
    fn new(
        shm: &wl_shm::WlShm,
        queue_handle: &QueueHandle<LockState>,
        output_global_name: u32,
        width: u32,
        height: u32,
        byte_len: usize,
    ) -> Result<Self, String> {
        let byte_len_i32 = i32::try_from(byte_len)
            .map_err(|_| "lock surface buffer is too large for Wayland".to_string())?;
        let stride = width
            .checked_mul(4)
            .and_then(|stride| i32::try_from(stride).ok())
            .ok_or_else(|| "lock surface stride is too large".to_string())?;

        let file = tempfile::tempfile()
            .map_err(|error| format!("could not create a private graphics buffer: {error}"))?;
        file.set_len(u64::try_from(byte_len).map_err(|_| "lock buffer is too large".to_string())?)
            .map_err(|error| format!("could not size the graphics buffer: {error}"))?;

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
            file,
            pool,
            buffer,
            width,
            height,
            byte_len,
            released: false,
        })
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), String> {
        if frame.len() != self.byte_len {
            return Err("lock frame has an unexpected byte length".to_string());
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("could not rewind the lock buffer: {error}"))?;
        self.file
            .write_all(frame)
            .map_err(|error| format!("could not paint the lock buffer: {error}"))
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

const fn graphics_budget_permits(resident_bytes: usize, requested_bytes: usize) -> bool {
    resident_bytes.saturating_add(requested_bytes) <= MAX_GRAPHICS_BYTES
}

fn release_keyboard(keyboard: &wl_keyboard::WlKeyboard) {
    if keyboard.version() >= 3 {
        keyboard.release();
    }
}

fn release_seat(seat: &wl_seat::WlSeat) {
    if seat.version() >= 5 {
        seat.release();
    }
}

fn release_output(output: &wl_output::WlOutput) {
    if output.version() >= 3 {
        output.release();
    }
}

impl LockState {
    fn handle_registry_global(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        interface: &str,
        version: u32,
        queue_handle: &QueueHandle<Self>,
    ) {
        match interface {
            "wl_compositor" if self.compositor.is_none() => {
                self.compositor = Some(registry.bind(name, version.min(4), queue_handle, ()));
            }
            "wl_shm" if self.shm.is_none() => {
                self.shm = Some(registry.bind(name, version.min(1), queue_handle, ()));
            }
            "wl_seat" if self.seats.len() < MAX_SEATS => {
                self.add_seat(registry, name, version, queue_handle);
            }
            "wl_seat" => {
                eprintln!("genoa-lock: ignoring seat {name}; reached the {MAX_SEATS}-seat limit");
            }
            "wl_output" if self.outputs.len() < MAX_OUTPUTS => {
                self.add_output(registry, name, version, queue_handle);
            }
            "wl_output" => eprintln!(
                "genoa-lock: output {name} exceeds the {MAX_OUTPUTS}-output limit; \
                 relying on the compositor's locked-output fallback"
            ),
            "ext_session_lock_manager_v1" if self.lock_manager.is_none() => {
                self.lock_manager = Some(registry.bind(name, version.min(1), queue_handle, ()));
            }
            _ => {}
        }
    }

    fn add_seat(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        version: u32,
        queue_handle: &QueueHandle<Self>,
    ) {
        match KeyboardInput::new() {
            Ok(input) => {
                let seat =
                    registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), queue_handle, ());
                self.seats.push(Seat {
                    global_name: name,
                    proxy: seat,
                    keyboard: None,
                    input,
                });
                if self.lock.is_none() {
                    self.failure = None;
                }
            }
            Err(error) if self.lock.is_some() || !self.seats.is_empty() => {
                Self::report_runtime_error(format!(
                    "could not initialize input for seat {name}: {error}"
                ));
            }
            Err(error) => self.failure = Some(error),
        }
    }

    fn add_output(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        version: u32,
        queue_handle: &QueueHandle<Self>,
    ) {
        let output =
            registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), queue_handle, ());
        self.outputs.push(Output {
            global_name: name,
            output,
        });

        if self.lock.is_some() {
            let output = self.outputs.last().map(|item| item.output.clone());
            if let Some(output) = output
                && let Err(error) = self.add_lock_surface(name, &output, queue_handle)
            {
                Self::report_runtime_error(error);
            }
        }
    }

    fn handle_registry_remove(&mut self, name: u32) {
        if let Some(index) = self.seats.iter().position(|seat| seat.global_name == name) {
            let mut removed = self.seats.swap_remove(index);
            if let Some(keyboard) = removed.keyboard.take() {
                release_keyboard(&keyboard);
            }
            release_seat(&removed.proxy);
        }
        if let Some(index) = self
            .surfaces
            .iter()
            .position(|surface| surface.output_global_name == name)
        {
            let removed = self.surfaces.swap_remove(index);
            removed.session_surface.destroy();
            removed.surface.destroy();
        }
        if let Some(index) = self
            .outputs
            .iter()
            .position(|output| output.global_name == name)
        {
            let removed = self.outputs.swap_remove(index);
            release_output(&removed.output);
        }
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
            } => state.handle_registry_global(registry, name, &interface, version, queue_handle),
            wl_registry::Event::GlobalRemove { name } => state.handle_registry_remove(name),
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
            if let Some(active) = state.surfaces[index].buffer.as_mut()
                && active.buffer == *released_buffer
            {
                active.released = true;
            }
            if state.surfaces[index]
                .retired_buffer
                .as_ref()
                .is_some_and(|buffer| buffer.buffer == *released_buffer)
                && let Some(mut released) = state.surfaces[index].retired_buffer.take()
            {
                released.released = true;
                state.surfaces[index].spare_buffer = Some(released);
            }
            if state.surfaces[index].pending_configure.is_some()
                && let Err(error) = state.render_pending_configure(index, queue_handle)
            {
                Self::report_runtime_error(error);
            } else if state.surfaces[index].redraw_pending {
                // A buffer release is delivered from a Wayland callback. Only
                // schedule the repaint here; creating/attaching a new buffer
                // during dispatch used to interfere with keyboard delivery.
                state.request_visual_redraw_now();
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
            && let Some(known_seat) = state.seats.iter_mut().find(|known| known.proxy == *seat)
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                if known_seat.keyboard.is_none() {
                    known_seat.keyboard =
                        Some(seat.get_keyboard(queue_handle, known_seat.global_name));
                }
            } else if let Some(keyboard) = known_seat.keyboard.take() {
                release_keyboard(&keyboard);
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

        if state.authentication.is_none() {
            let redraw = !matches!(&action, Some(KeyAction::Ignore) | None);
            match action {
                Some(KeyAction::Backspace) => state.password.backspace(),
                Some(KeyAction::Submit) => state.submit_password(),
                Some(KeyAction::Text(text)) => state.password.push_text(&text),
                Some(KeyAction::Ignore) | None => {}
            }
            if redraw {
                state.request_visual_redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_GRAPHICS_BYTES, checked_buffer_len, graphics_budget_permits};

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

    #[test]
    fn graphics_budget_accounts_for_the_replacement_peak() {
        let buffer = 64 * 1024 * 1024;
        assert!(graphics_budget_permits(
            MAX_GRAPHICS_BYTES - 2 * buffer,
            2 * buffer
        ));
        assert!(!graphics_budget_permits(
            MAX_GRAPHICS_BYTES - buffer,
            2 * buffer
        ));
    }
}
