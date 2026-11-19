"""Хелперы e2e: вызов метода фронтенда xdg-desktop-portal и ожидание Request.Response."""
import os
import sys

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402


def bus():
    return Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call_request(conn, iface, method, fmt, args, opts, timeout=15):
    """Вызывает фронтенд-метод, возвращающий handle запроса, и ждёт Request.Response.
    fmt — сигнатура позиционных аргументов без options, например "ss"; args — их значения;
    opts — dict {имя: GLib.Variant}; handle_token добавляется автоматически.
    Возвращает (response:int, results:dict)."""
    token = f"t{os.getpid()}x{abs(hash((iface, method, fmt, repr(args)))) % 10**6}"
    sender = conn.get_unique_name()[1:].replace(".", "_")
    handle = f"/org/freedesktop/portal/desktop/request/{sender}/{token}"
    loop = GLib.MainLoop()
    out = {}

    def on_resp(_c, _s, _p, _i, _n, params):
        out["r"] = (params.get_child_value(0).unpack(), params.get_child_value(1).unpack())
        loop.quit()

    sub = conn.signal_subscribe(
        "org.freedesktop.portal.Desktop", "org.freedesktop.portal.Request", "Response",
        handle, None, Gio.DBusSignalFlags.NONE, on_resp)
    GLib.timeout_add_seconds(timeout, loop.quit)
    opts = dict(opts)
    opts["handle_token"] = GLib.Variant("s", token)
    full = GLib.Variant(f"({fmt}a{{sv}})", (*args, opts))
    conn.call_sync("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
                   iface, method, full, None, Gio.DBusCallFlags.NONE, 10000, None)
    loop.run()
    conn.signal_unsubscribe(sub)
    if "r" not in out:
        raise TimeoutError(f"{iface}.{method}: Response не пришёл за {timeout} с")
    return out["r"]


def serve(conn, name, path, xml, handler):
    """Поднимает мок-сервис: handler(method, params) -> GLib.Variant|None."""
    info = Gio.DBusNodeInfo.new_for_xml(xml)

    def on_call(_c, _s, _p, _i, method, params, inv):
        try:
            res = handler(method, params)
            inv.return_value(res)
        except Exception as e:  # noqa: BLE001
            inv.return_dbus_error("org.freedesktop.DBus.Error.Failed", str(e))

    conn.register_object(path, info.interfaces[0], on_call, None, None)
    Gio.bus_own_name_on_connection(conn, name, Gio.BusNameOwnerFlags.NONE, None, None)


def pump(ms=300):
    loop = GLib.MainLoop()
    GLib.timeout_add(ms, loop.quit)
    loop.run()


def check(cond, msg):
    print(("ok   " if cond else "FAIL ") + msg)
    if not cond:
        global failed
        failed = True


failed = False


def finish():
    sys.exit(1 if failed else 0)
