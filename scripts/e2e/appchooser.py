"""e2e Фазы 4: impl.AppChooser демона → conchy-files (D-Bus-активация). Вызов идёт прямо к демону,
чтобы xdg-desktop-portal не запускал выбранные приложения на машине разработчика."""
import os, sys, time, glob
sys.path.insert(0, os.path.dirname(__file__))
from portal import *  # noqa: F401,F403
from gi.repository import Gio, GLib

conn = bus()
ids = []
for f in sorted(glob.glob("/usr/share/applications/*.desktop")):
    t = open(f, errors="ignore").read()
    if "NoDisplay=true" in t or "Hidden=true" in t or "Exec=" not in t:
        continue
    ids.append(os.path.basename(f)[:-8])
    if len(ids) == 3:
        break
check(len(ids) == 3, f"нашлись 3 приложения: {ids}")

def call(choices, last, keys, handle):
    out = {}
    loop = GLib.MainLoop()
    def done(c, res):
        try:
            out["r"] = c.call_finish(res).unpack()
        except Exception as e:
            out["err"] = str(e)
        loop.quit()
    opts = {"last_choice": GLib.Variant("s", last), "content_type": GLib.Variant("s", "text/plain"),
            "filename": GLib.Variant("s", "notes.txt")}
    conn.call(
        "org.freedesktop.impl.portal.desktop.conchy", "/org/freedesktop/portal/desktop",
        "org.freedesktop.impl.portal.AppChooser", "ChooseApplication",
        GLib.Variant("(ossasa{sv})", (handle, "org.test", "", choices, opts)),
        None, Gio.DBusCallFlags.NONE, 60000, None, done)
    def press():
        for _ in range(80):
            if hypr_has("conchy-files-appchooser"):
                break
            time.sleep(0.1)
        time.sleep(1.2)
        for k in keys:
            hypr_key(k, "conchy-files-appchooser")
            time.sleep(0.15)
    after(300, press)
    GLib.timeout_add_seconds(50, loop.quit)
    loop.run()
    return out

r = call(ids, "", ["Down", "Return"], "/r/ac1")
code, res = r.get("r", (None, {}))
check(code == 0 and res.get("choice") == ids[1], f"Down+Return → второе приложение: {r}")

r = call(ids, ids[2], ["Return"], "/r/ac2")
code, res = r.get("r", (None, {}))
check(code == 0 and res.get("choice") == ids[2], f"last_choice выделен первым: {r}")

r = call(ids, "", ["Escape"], "/r/ac3")
code, res = r.get("r", (None, {}))
check(code == 1 and not res, f"Escape → response 1: {r}")
finish()
