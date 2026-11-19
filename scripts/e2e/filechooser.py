"""e2e Фазы 3: FileChooser через настоящий xdp, демон и conchy-files (D-Bus-активация).
Требует Wayland-сессию и hyprctl (клавиши шлются в окно через Hyprland)."""
import os, sys, tempfile, time
sys.path.insert(0, os.path.dirname(__file__))
from portal import *  # noqa: F401,F403
from gi.repository import GLib

conn = bus()
d = tempfile.mkdtemp(prefix="pick-e2e-")
for n in ("a.png", "b.jpg", "c.txt"):
    open(os.path.join(d, n), "w").close()
os.mkdir(os.path.join(d, "Docs"))
IFACE = "org.freedesktop.portal.FileChooser"
filters = GLib.Variant("a(sa(us))", [("Images", [(0, "*.png")]), ("All", [(0, "*")])])
folder = GLib.Variant("ay", list(d.encode()) + [0])

def press_when_open(keys, delay=1500):
    def go():
        for _ in range(60):
            if hypr_has("conchy-files-picker"):
                break
            time.sleep(0.1)
        time.sleep(delay / 1000)
        for k in keys:
            hypr_key(k)
            time.sleep(0.15)
    after(300, go)

# 1. Выбор файла: фильтр Images → видны Docs/ и a.png; down,down → a.png; Return.
press_when_open(["Down", "Down", "Return"])
t0 = time.time()
r, res = call_request(conn, IFACE, "OpenFile", "ss", ("", "Pick"),
                      {"filters": filters, "current_folder": folder}, timeout=40)
check(r == 0, f"OpenFile response=0 (got {r}), {time.time()-t0:.1f}s")
uris = res.get("uris", [])
check(len(uris) == 1 and uris[0].endswith("/a.png") and d in uris[0], f"uris={uris}")

# 2. Отмена по Escape.
press_when_open(["Escape"], 800)
r, res = call_request(conn, IFACE, "OpenFile", "ss", ("", "Cancel me"),
                      {"current_folder": folder}, timeout=40)
check(r == 1, f"Escape → response=1 (got {r})")
check(not res.get("uris"), "нет uris при отмене")

# 3. Сохранение: имя из current_name, Return → файл не существует → response 0.
press_when_open(["Return"], 800)
r, res = call_request(conn, IFACE, "SaveFile", "ss", ("", "Save"),
                      {"current_name": GLib.Variant("s", "new file.txt"), "current_folder": folder}, timeout=40)
check(r == 0, f"SaveFile response=0 (got {r})")
uris = res.get("uris", [])
check(len(uris) == 1 and uris[0].endswith("/new%20file.txt"), f"save uris={uris}")

# 4. Сохранение поверх существующего: Return → подтверждение → Return → response 0.
press_when_open(["Return", "Return"], 800)
r, res = call_request(conn, IFACE, "SaveFile", "ss", ("", "Overwrite"),
                      {"current_name": GLib.Variant("s", "c.txt"), "current_folder": folder}, timeout=40)
check(r == 0 and res.get("uris", [""])[0].endswith("/c.txt"), f"overwrite {r} {res.get('uris')}")
finish()
