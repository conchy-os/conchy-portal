"""e2e Фазы 6: xdp (фронтенд) → демон → пикер в conchy-shell → мок conchy-wm.
Клавиши пикера (оверлей) шлются через Hyprland без указания окна."""
import os, sys, time, subprocess
sys.path.insert(0, os.path.dirname(__file__))
from portal import *  # noqa: F401,F403
from mock_wm import MockWm
from gi.repository import Gio, GLib

conn = bus()
wm = MockWm(conn, {42: ("firefox.desktop", "Video call"), 43: ("alacritty.desktop", "term")})
pump(500)
IFACE = "org.freedesktop.portal.ScreenCast"
shot = os.environ.get("CAST_SHOT")

def key(k):
    subprocess.run(["hyprctl", "dispatch", f'hl.dsp.send_shortcut({{ mods = "", key = "{k}" }})'], stdout=subprocess.DEVNULL)

def layer_open(ns):
    import json
    out = subprocess.run(["hyprctl", "layers", "-j"], capture_output=True, text=True).stdout
    return ns in out

def keys_later(keys, delay=0.8, take_shot=False):
    def go():
        for _ in range(150):  # шелл поднимается активацией: ждём, пока появится оверлей
            if layer_open("portal-cast-picker"):
                break
            time.sleep(0.1)
        time.sleep(delay)
        if take_shot and shot:
            subprocess.run(["grim", shot])
        for k in keys:
            key(k)
            time.sleep(0.2)
    after(300, go)

def create(token):
    r, res = call_request(conn, IFACE, "CreateSession", "", (), {"session_handle_token": GLib.Variant("s", token)})
    check(r == 0 and "session_handle" in res, f"CreateSession: {r} {res}")
    return res["session_handle"]

def select(sess, opts):
    r, _ = call_request(conn, IFACE, "SelectSources", "o", (sess,), opts)
    check(r == 0, f"SelectSources response={r}")

# 1. Пикер: Space выбирает первую строку, Return делится.
sess = create("cast1")
select(sess, {"types": GLib.Variant("u", 3), "persist_mode": GLib.Variant("u", 2), "cursor_mode": GLib.Variant("u", 2)})
keys_later(["Space", "Return"], take_shot=True)
r, res = call_request(conn, IFACE, "Start", "os", (sess, ""), {}, timeout=40)
check(r == 0, f"Start response=0 (got {r}) {res}")
streams = res.get("streams", [])
check(len(streams) == 1 and streams[0][0] > 500, f"streams={streams}")
# Фронтенд xdp сам хранит restore_data и отдаёт приложению только restore_token.
check(bool(res.get("restore_token")), f"restore_token от фронтенда: {res.get('restore_token')}")
check("record_monitor eDP-1 cursor=1" in wm.log, f"курсор портала 2 → Mutter 1; лог wm: {wm.log}")
token = res.get("restore_token")

# 2. Закрытие сессии из приложения гасит поток в wm.
conn.call_sync("org.freedesktop.portal.Desktop", sess, "org.freedesktop.portal.Session", "Close", None, None,
               Gio.DBusCallFlags.NONE, 5000, None)
pump(800)
check(wm.log[-1] == "stop", f"Session.Close → Stop в wm: {wm.log[-3:]}")

# 3. Restore-токен: пикер не показывается (клавиш никто не шлёт).
sess2 = create("cast2")
select(sess2, {"types": GLib.Variant("u", 3), "persist_mode": GLib.Variant("u", 2),
               "restore_token": GLib.Variant("s", token)})
r, res = call_request(conn, IFACE, "Start", "os", (sess2, ""), {}, timeout=15)
check(r == 0 and len(res.get("streams", [])) == 1, f"Start по restore без пикера: {r} {res}")
conn.call_sync("org.freedesktop.portal.Desktop", sess2, "org.freedesktop.portal.Session", "Close", None, None,
               Gio.DBusCallFlags.NONE, 5000, None)

# 4. Отмена в пикере (Esc) → response 1.
sess3 = create("cast3")
select(sess3, {"types": GLib.Variant("u", 3)})
keys_later(["Escape"])
r, res = call_request(conn, IFACE, "Start", "os", (sess3, ""), {}, timeout=40)
check(r == 1, f"Esc в пикере → response=1 (got {r})")
finish()
