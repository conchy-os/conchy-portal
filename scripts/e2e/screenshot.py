"""e2e Фазы 2: Screenshot/PickColor через настоящий xdp (мок conchy-wm на шине)."""
import os, sys, tempfile
sys.path.insert(0, os.path.dirname(__file__))
from portal import *  # noqa: F401,F403
from gi.repository import GLib

conn = bus()
shot = os.path.join(tempfile.mkdtemp(), "Screenshots", "wm shot.png")
XML = """<node><interface name="org.gnome.Shell.Screenshot">
<method name="Screenshot"><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="s" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
<method name="PickColor"><arg type="a{sv}" direction="out"/></method></interface></node>"""

def handler(method, params):
    if method == "Screenshot":
        os.makedirs(os.path.dirname(shot), exist_ok=True)
        open(shot, "wb").write(b"\x89PNG\r\n\x1a\nfake")
        return GLib.Variant("(bs)", (True, shot))
    return GLib.Variant("(a{sv})", ({"color": GLib.Variant("(ddd)", (0.25, 0.5, 0.75))},))

serve(conn, "org.gnome.Shell.Screenshot", "/org/gnome/Shell/Screenshot", XML, handler)
pump(300)

# Интерактивный снимок: фронтенд xdp не спрашивает разрешение (это делает Фаза 5 для
# неинтерактивного), бэкенд запускает фейковый conchy-shot.
r, res = call_request(conn, "org.freedesktop.portal.Screenshot", "Screenshot",
                      "s", ("",), {"interactive": GLib.Variant("b", True)})
check(r == 0, f"Screenshot(interactive) response=0 (got {r})")
uri = res.get("uri", "")
check(uri.startswith("file://") and uri.endswith(".png"), f"uri={uri}")
path = uri[len("file://"):]
check(os.path.exists(path) and open(path, "rb").read().startswith(b"\x89PNG"), "файл-PNG существует")

# Отмена: conchy-shot вышел с кодом 1 → response 1.
cancel = os.path.join(os.path.dirname(os.environ["CONCHY_SHOT_BIN"]), "cancel")
open(cancel, "w").close()
r, res = call_request(conn, "org.freedesktop.portal.Screenshot", "Screenshot",
                      "s", ("",), {"interactive": GLib.Variant("b", True)})
check(r == 1, f"Screenshot cancelled response=1 (got {r})")
os.remove(cancel)

r, res = call_request(conn, "org.freedesktop.portal.Screenshot", "PickColor",
                      "s", ("",), {})
check(r == 0 and tuple(res.get("color", ())) == (0.25, 0.5, 0.75), f"PickColor {r} {res}")
finish()
