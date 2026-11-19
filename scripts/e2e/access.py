"""e2e Фазы 5: impl.Access демона → conchy-shell (активация, режим CONCHY_SHELL_PORTAL_ONLY).
Вызов идёт прямо к демону. Клавиши — через Hyprland (слой-оверлей с эксклюзивным фокусом)."""
import os, sys, time, subprocess
sys.path.insert(0, os.path.dirname(__file__))
from portal import *  # noqa: F401,F403
from gi.repository import Gio, GLib

conn = bus()
shot = os.environ.get("ACCESS_SHOT")  # путь для скриншота диалога (необязательно)

def key(k):
    subprocess.run(["hyprctl", "dispatch", f'hl.dsp.send_shortcut({{ mods = "", key = "{k}" }})'],
                   stdout=subprocess.DEVNULL)

def call(handle, keys, take_shot=False):
    out = {}
    loop = GLib.MainLoop()
    def done(c, res):
        try:
            out["r"] = c.call_finish(res).unpack()
        except Exception as e:
            out["err"] = str(e)
        loop.quit()
    choices = [("remember", "Запомнить выбор", [], "false")]
    opts = {"grant_label": GLib.Variant("s", "Разрешить"), "deny_label": GLib.Variant("s", "Запретить"),
            "choices": GLib.Variant("a(ssa(ss)s)", choices)}
    conn.call("org.freedesktop.impl.portal.desktop.conchy", "/org/freedesktop/portal/desktop",
              "org.freedesktop.impl.portal.Access", "AccessDialog",
              GLib.Variant("(osssssa{sv})", (handle, "org.mozilla.firefox", "", "Разрешить доступ к камере?",
                                              "Firefox хочет использовать камеру", "Доступ можно отозвать позже.", opts)),
              None, Gio.DBusCallFlags.NONE, 60000, None, done)
    def press():
        time.sleep(3.0)
        if take_shot and shot:
            subprocess.run(["grim", shot])
        for k in keys:
            key(k)
            time.sleep(0.2)
    after(300, press)
    GLib.timeout_add_seconds(50, loop.quit)
    loop.run()
    return out

r = call("/r/ax1", ["Return"], take_shot=True)
code, res = r.get("r", (None, {}))
check(code == 1 and not res, f"Enter по умолчанию = «Запретить» (1): {r}")

r = call("/r/ax2", ["Tab", "Return"])
code, res = r.get("r", (None, {}))
check(code == 0 and res.get("choices") == [("remember", "false")], f"Tab+Enter = «Разрешить» (0): {r}")

r = call("/r/ax3", ["Escape"])
code, res = r.get("r", (None, {}))
check(code == 1, f"Escape = отказ (1): {r}")
finish()
