#!/usr/bin/env bash
# Поднимает изолированное окружение: приватная шина + настоящий xdg-desktop-portal с нашим бэкендом.
#   source scripts/e2e-env.sh [debug|release]
# Экспортирует E2E_DIR, XDG_CONFIG_HOME (пустой конфиг conchy), DBUS_SESSION_BUS_ADDRESS, E2E_PIDS.
# Остановка: e2e_stop
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="${1:-debug}"
PRIVBUS_DIR="$(mktemp -d)"
E2E_DIR="$PRIVBUS_DIR"
mkdir -p "$E2E_DIR/portals" "$E2E_DIR/config/conchy" "$E2E_DIR/config/xdg-desktop-portal" "$E2E_DIR/data"
cp /usr/share/xdg-desktop-portal/portals/*.portal "$E2E_DIR/portals/"
cp "$root/data/conchy.portal" "$E2E_DIR/portals/"
cp "$root/data/conchy-portals.conf" "$E2E_DIR/config/xdg-desktop-portal/"
# Фейковый conchy-shot: пишет «PNG» в --portal-output (или выходит с кодом 1, если FAKE_SHOT_CANCEL).
cat > "$E2E_DIR/fake-conchy-shot" <<'FAKE'
#!/bin/sh
while [ $# -gt 0 ]; do [ "$1" = --portal-output ] && out="$2"; shift; done
[ -e "$(dirname "$0")/cancel" ] && exit 1
printf '\211PNG\r\n\032\nfake' > "$out"
FAKE
chmod +x "$E2E_DIR/fake-conchy-shot"
export CONCHY_SHOT_BIN="$E2E_DIR/fake-conchy-shot"
# Окружение — до запуска dbus-daemon: активируемые сервисы наследуют его.
export XDG_CONFIG_HOME="$E2E_DIR/config" XDG_DESKTOP_PORTAL_DIR="$E2E_DIR/portals" XDG_CURRENT_DESKTOP=conchy
source "$root/scripts/private-bus.sh" "$profile"
/usr/lib/xdg-desktop-portal -r >"$E2E_DIR/xdp.log" 2>&1 &
E2E_XDP_PID=$!
e2e_stop() {
  kill "$E2E_XDP_PID" 2>/dev/null
  pkill -TERM -P "$$" -f 'target/(debug|release)/xdg-desktop-portal-conchy' 2>/dev/null
  kill "$PRIVBUS_PID" 2>/dev/null
}
for _ in $(seq 50); do busctl --user list 2>/dev/null | grep -q '^org.freedesktop.portal.Desktop' && break; sleep 0.1; done
