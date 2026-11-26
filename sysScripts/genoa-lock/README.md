# genoa-lock

`genoa-lock` is an intentionally small, experimental Wayland session locker.
It is a clean-room Rust implementation: no gtklock source or plugin ABI is
included.

## Current scope

- Locks every Wayland output through `ext-session-lock-v1`.
- Draws an opaque, darkened embedded wallpaper before accepting unlock input.
- Shows a local clock, a password box with dots, and a bounded/sanitized read
  of Genoa's existing weather cache. It never executes a weather command.
- Authenticates only the effective user's account through the dedicated
  `genoa-lock` PAM service.
- Supports only one optional action: `--suspend-after-lock`, which invokes the
  fixed executable `/usr/bin/systemctl suspend` after the compositor confirms
  the session is locked.
- Owns a private per-session instance guard, so overlapping hotkey, sidebar,
  Waybar, or idle requests cannot compete for keyboard focus.
- Reuses one released shared-memory buffer per output and applies a global
  graphics-memory budget.

It deliberately has no themes, module loader, shell commands, status files,
network access, or user-supplied executable configuration.

The first prototype handles the compositor's supplied XKB layout and visual
password dots, but does not yet implement compose keys or input methods.

## Security status

This is a prototype being integrated for hardware testing. Genoa's installer
installs the release-built binary as root-owned `/usr/local/bin/genoa-lock`,
installs its dedicated PAM policy, and then migrates the known Sway and Niri
lock launch sites. It keeps GTKLock installed if it detects an active custom
GTKLock command it cannot rewrite safely.

Before treating it as a generally supported production locker, the integration
test plan must still cover the target compositors, multiple displays,
hot-plugging, suspend/resume, PAM failures, and recovery from a compositor or
locker crash.

The protocol binding is currently exposed by `wayland-protocols` as a staging
protocol. That is another reason to keep this isolated until it has received
real hardware testing.

## PAM

The program intentionally uses its own PAM service name, rather than silently
falling back to `login`. Packaging must install and review `/etc/pam.d/genoa-lock`
before this program is usable. A suggested Arch baseline, subject to a separate
PAM review, is:

```text
auth      include system-local-login
account   include system-local-login
```

The locker's code does not open a PAM session: it only authenticates the owner
of the already-running desktop session.

Build/runtime dependencies are `libpam` and `libxkbcommon`; the latter is
loaded dynamically. There is no `libclang` or bindgen dependency in the build.

## Manual prototype invocation

```sh
cargo run --release
cargo run --release -- --suspend-after-lock
```

There is intentionally no escape-to-exit shortcut after a lock has succeeded.
