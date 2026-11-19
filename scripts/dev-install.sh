#!/usr/bin/env bash
# Dev-установка бэкенда портала в пользовательские каталоги (без root).
#
#   scripts/dev-install.sh [--release] [--uninstall]
#
# Ставит:
#   ~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.conchy.service (Exec -> target/…)
#   ~/.local/share/xdg-desktop-portal/portals/conchy.portal
#   ~/.config/xdg-desktop-portal/conchy-portals.conf
#
# xdg-desktop-portal ищет .portal-файлы в $XDG_DESKTOP_PORTAL_DIR, а если переменная не задана —
# только в /usr/share/xdg-desktop-portal/portals. Поэтому после установки фронтенду нужно
# передать переменную через окружение активации D-Bus (скрипт делает это сам) и перезапустить его:
#   dbus-update-activation-environment --systemd XDG_DESKTOP_PORTAL_DIR=...
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
profile=debug
uninstall=0
for a in "$@"; do
  case "$a" in
    --release) profile=release ;;
    --uninstall) uninstall=1 ;;
    *) echo "неизвестный аргумент: $a" >&2; exit 2 ;;
  esac
done

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
conf_home="${XDG_CONFIG_HOME:-$HOME/.config}"
svc_dir="$data_home/dbus-1/services"
portal_dir="$data_home/xdg-desktop-portal/portals"
conf_dir="$conf_home/xdg-desktop-portal"
name=org.freedesktop.impl.portal.desktop.conchy

if [ "$uninstall" = 1 ]; then
  rm -f "$svc_dir/$name.service" "$portal_dir/conchy.portal" "$conf_dir/conchy-portals.conf"
  echo "удалено"
  exit 0
fi

if [ "$profile" = release ]; then
  cargo build --manifest-path "$root/Cargo.toml" --release
else
  cargo build --manifest-path "$root/Cargo.toml"
fi
bin="$root/target/$profile/xdg-desktop-portal-conchy"

mkdir -p "$svc_dir" "$portal_dir" "$conf_dir"
sed "s|^Exec=.*|Exec=$bin|" "$root/data/$name.service" > "$svc_dir/$name.service"
install -m644 "$root/data/conchy.portal" "$portal_dir/conchy.portal"
install -m644 "$root/data/conchy-portals.conf" "$conf_dir/conchy-portals.conf"

# Системные .portal остаются видимыми через общий каталог — копируем их рядом, иначе
# XDG_DESKTOP_PORTAL_DIR скроет gnome/gtk бэкенды.
for f in /usr/share/xdg-desktop-portal/portals/*.portal; do
  [ -e "$portal_dir/$(basename "$f")" ] || ln -s "$f" "$portal_dir/$(basename "$f")"
done

export XDG_DESKTOP_PORTAL_DIR="$portal_dir"
dbus-update-activation-environment XDG_DESKTOP_PORTAL_DIR 2>/dev/null || true
pkill -f '(^|/)xdg-desktop-portal$' 2>/dev/null || true
pkill -f xdg-desktop-portal-conchy 2>/dev/null || true
echo "установлено: $bin"
echo "XDG_DESKTOP_PORTAL_DIR=$portal_dir (должна быть в окружении xdg-desktop-portal)"
