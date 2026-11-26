# AGENTS.md — conchy-portal

Instructions for AI coding agents working in `conchy-portal`.

## Shared Conchy OS Rules

- **Rolling release**: no version numbers in user-facing specs, changelogs, or design decisions.
- **Rust-only for our own code**: no GTK or Qt in Conchy components.
- **One utility = one task**: `xdg-desktop-portal-conchy` is a small headless D-Bus portal router; UI dialogs live in existing warm Conchy processes (`conchy-files`, `conchy-shell`), and screen/capture primitives live in `conchy-wm` / `conchy-shot`.
- **Target OS priority**: Void Linux (`glibc` / `runit` / `xbps`) first, Arch Linux second.
- **Git workflow**: single long-lived branch `main`; work on short PR branches and delete them after merge.
- **Roadmap workflow**: keep progress synchronized with `../conchy-os/ROADMAP.md` and `../conchy-os/roadmap.json` when executing roadmap tasks (do not modify sibling repositories unless explicitly tasked to update the roadmap).
- **Mandatory Rust verification** after every code change:
  ```sh
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

## Safety Rules

- **Never touch the live user session**: do not invoke `hyprctl`, do not start daemons on the host session D-Bus, and do not write to `~/.config` or `~/.local`.
- **Isolate XDG directories**: whenever running tools, scripts, or tests that read/write config or portal state, isolate `XDG_CONFIG_HOME` (and `XDG_DATA_HOME`) in a temporary directory created with `mktemp -d`. Remember the `dconf`/`gsettings` trap: GLib/portal tools can fall back to user config/data paths if XDG variables are not isolated.
- **Never use `pkill -f` matching your own command** (a broad `pkill -f` pattern can match the calling shell or command line itself).
- **Never load unverified `.so` files**.

## Repository Specifics (`conchy-portal`)

### Workspace Layout

Cargo workspace (`edition = "2024"`, `resolver = "3"`):

- `crates/proto` (`conchy-portal-proto`): D-Bus constants and flat request/result types shared between the portal daemon and UI hosts. **Intentionally depends only on `serde` and `zvariant` (no `zbus`, `tokio`, or `gpui`)** so `conchy-files` and `conchy-shell` can depend on it via path without dependency version conflicts.
- `crates/daemon` (`xdg-desktop-portal-conchy`): headless daemon implementing `org.freedesktop.impl.portal.desktop.conchy` at `/org/freedesktop/portal/desktop`. Depends on `conchy-portal-proto` and path dependency `conchy-config` (`../conchy-config`).
  - `Settings`: served directly from `$XDG_CONFIG_HOME/conchy` (`theme.kdl` + `conchy-config`, watched via `notify`).
  - `Screenshot`, `PickColor`: delegates to `org.gnome.Shell.Screenshot` (`conchy-wm`) or `conchy-shot --portal-output` (`CONCHY_SHOT_BIN`).
  - `FileChooser`, `AppChooser`: delegates to `org.conchy.Portal.FilesHost` (`conchy-files`).
  - `Access`, `ScreenCast` (picker): delegates to `org.conchy.Portal.ShellHost` (`conchy-shell`); `ScreenCast` PipeWire streams use `org.gnome.Mutter.ScreenCast` (`conchy-wm`).
  - `Inhibit`, `Notification`: forward to `org.freedesktop.ScreenSaver` / `org.freedesktop.Notifications`.
- `data/`:
  - `conchy.portal` — backend descriptor (`UseIn=conchy`).
  - `conchy-portals.conf` — routes `Settings`, `Screenshot`, `FileChooser`, `AppChooser`, `Access` to `conchy;`; keeps `default=gnome;gtk;`, `Notification=gtk;`, `Secret=oo7-portal;gnome-keyring;` (note: `ScreenCast` remains on `gnome` and `Inhibit` is not enabled in config until verified against real apps).
  - `org.freedesktop.impl.portal.desktop.conchy.service` and `org.conchy.Portal.FilesHost.service` — D-Bus service files.
- `scripts/`: isolated test helpers and integration scripts (see warnings below).
- `packaging/void/xdg-desktop-portal-conchy/template`: **unbuilt draft** `xbps-src` template for task 1.2.1 (notes the `../conchy-config` path dependency).
- `PLAN.md`: architecture specification and decision journal.

### Code Style & Comment Language

- **Comment language**: **Russian** for code comments, doc comments, and `PLAN.md` entries (matching `crates/` and `PLAN.md`).
- Keep existing comments and docstrings unrelated to your changes intact.
- Keep `crates/proto` free of `zbus`, `tokio`, `gpui`, or UI dependencies.

### Commands

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- `cargo test --workspace` runs unit and integration tests using isolated in-memory p2p `zbus` `UnixStream` pairs and temporary directories — it does not require a live `xdg-desktop-portal`, `dbus-daemon`, or Wayland compositor.

### Scripts & Warnings

- **Safe isolated bus / xdp helpers**:
  - `scripts/private-bus.sh [debug|release]` starts an isolated `dbus-daemon` in `mktemp -d` with private `.service` definitions.
  - `scripts/e2e-env.sh [debug|release]` builds on `private-bus.sh`, isolates `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, and `XDG_DESKTOP_PORTAL_DIR` in `mktemp -d`, creates a fake `conchy-shot`, sets `XDG_CURRENT_DESKTOP=conchy`, and starts `/usr/lib/xdg-desktop-portal -r`. Stop it with `e2e_stop`.
- **DO NOT run without explicit user permission**:
  - `scripts/e2e/access.py`, `scripts/e2e/screencast.py`, `scripts/e2e/filechooser.py`, `scripts/e2e/appchooser.py` — these scripts **inject synthetic keyboard events via `hyprctl` against the live Hyprland session**. Never run them autonomously.
  - `scripts/dev-install.sh` — **writes to `~/.local/share/...` and `~/.config/xdg-desktop-portal/`**, updates the live D-Bus activation environment (`dbus-update-activation-environment XDG_DESKTOP_PORTAL_DIR`), and kills running portal processes via `pkill -f`. Never run it autonomously.
- **Packaging draft**: `packaging/void/xdg-desktop-portal-conchy/template` is an unbuilt draft for task 1.2.1; do not assume it builds cleanly in `xbps-src` yet.
