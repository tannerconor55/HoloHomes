#!/usr/bin/env bash
# Wraps `hc-spin` to avoid two environment-specific Electron crashes seen in dev:
#
# 1. If ELECTRON_RUN_AS_NODE is set (common when this shell descends from another
#    Electron app, e.g. VS Code's integrated terminal), hc-spin's spawned `electron`
#    binary runs as plain Node instead of launching a GUI, and crashes trying to read
#    `electron.app.isPackaged` on the resulting non-Electron `electron` module.
# 2. On some Wayland sessions, Electron's GPU process segfaults outright (VA-API /
#    Vulkan-vs-wayland-ozone driver issues). Falling back to XWayland with software
#    GL rendering avoids it, at the cost of a little rendering performance — a fine
#    trade for a dev UI. This is scoped to Wayland sessions only, so a native X11
#    desktop (or a Wayland one that doesn't hit this) is left untouched.
set -euo pipefail

unset ELECTRON_RUN_AS_NODE

if [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
  unset WAYLAND_DISPLAY XDG_SESSION_TYPE
  export LIBGL_ALWAYS_SOFTWARE=1
fi

exec hc-spin "$@"
