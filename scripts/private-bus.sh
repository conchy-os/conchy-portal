#!/usr/bin/env bash
# Изолированная сессионная шина для ручных проверок (не трогает реальную сессию).
#   source scripts/private-bus.sh [debug|release]   # экспортирует DBUS_SESSION_BUS_ADDRESS, PRIVBUS_PID
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="${1:-debug}"
PRIVBUS_DIR="${PRIVBUS_DIR:-$(mktemp -d)}"
mkdir -p "$PRIVBUS_DIR/services"
sed "s|^Exec=.*|Exec=$root/target/$profile/xdg-desktop-portal-conchy|" \
  "$root/data/org.freedesktop.impl.portal.desktop.conchy.service" \
  > "$PRIVBUS_DIR/services/org.freedesktop.impl.portal.desktop.conchy.service"
# Хост conchy-files (FileChooser/AppChooser): бинарь можно подменить CONCHY_FILES_BIN.
files_bin="${CONCHY_FILES_BIN:-$root/../conchy-files/target/$profile/conchy-files}"
sed "s|^Exec=.*|Exec=$files_bin --service|" "$root/data/org.conchy.Portal.FilesHost.service" \
  > "$PRIVBUS_DIR/services/org.conchy.Portal.FilesHost.service"
# Хост шелла (Access, пикер ScreenCast). В настоящей сессии шелл запущен всегда и сам занимает имя;
# здесь поднимаем его по активации в режиме «только хост портала» (CONCHY_SHELL_PORTAL_ONLY).
shell_bin="${CONCHY_SHELL_BIN:-$root/../conchy-shell/target/$profile/conchy-shell}"
cat > "$PRIVBUS_DIR/services/org.conchy.Portal.ShellHost.service" <<SVC
[D-BUS Service]
Name=org.conchy.Portal.ShellHost
Exec=/usr/bin/env CONCHY_SHELL_PORTAL_ONLY=1 $shell_bin
SVC
# Хранилище разрешений xdp: restore-токены ScreenCast и кеш ответов Access (в хранилище данных
# приватного XDG_DATA_HOME теста, не в настоящем).
if [ -x /usr/lib/xdg-permission-store ]; then
  cat > "$PRIVBUS_DIR/services/org.freedesktop.impl.portal.PermissionStore.service" <<SVC
[D-BUS Service]
Name=org.freedesktop.impl.portal.PermissionStore
Exec=/usr/lib/xdg-permission-store
SVC
fi
# Хранилище разрешений xdp: restore-токены ScreenCast и кеш ответов Access. Пишет в
# XDG_DATA_HOME теста (e2e-env.sh), не в настоящий.
if [ -x /usr/lib/xdg-permission-store ]; then
  cat > "$PRIVBUS_DIR/services/org.freedesktop.impl.portal.PermissionStore.service" <<SVC
[D-BUS Service]
Name=org.freedesktop.impl.portal.PermissionStore
Exec=/usr/lib/xdg-permission-store
SVC
fi
cat > "$PRIVBUS_DIR/bus.conf" <<CONF
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$PRIVBUS_DIR</listen>
  <servicedir>$PRIVBUS_DIR/services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
exec 3< <(dbus-daemon --config-file="$PRIVBUS_DIR/bus.conf" --print-address=1 --print-pid=1 --nofork)
read -r DBUS_SESSION_BUS_ADDRESS <&3
read -r PRIVBUS_PID <&3
export DBUS_SESSION_BUS_ADDRESS PRIVBUS_PID PRIVBUS_DIR
