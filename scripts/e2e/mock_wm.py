"""Мок conchy-wm для e2e: org.gnome.Mutter.ScreenCast и org.gnome.Shell.Introspect на шине."""
import itertools
from gi.repository import Gio, GLib

SC_XML = """<node><interface name="org.gnome.Mutter.ScreenCast">
<method name="CreateSession"><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
</interface></node>"""
SESSION_XML = """<node><interface name="org.gnome.Mutter.ScreenCast.Session">
<method name="Start"/><method name="Stop"/>
<method name="RecordMonitor"><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<method name="RecordWindow"><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<signal name="Closed"/></interface></node>"""
STREAM_XML = """<node><interface name="org.gnome.Mutter.ScreenCast.Stream">
<signal name="PipeWireStreamAdded"><arg type="u"/></signal>
<property name="Parameters" type="a{sv}" access="read"/></interface></node>"""
INTRO_XML = """<node><interface name="org.gnome.Shell.Introspect">
<method name="GetWindows"><arg type="a{ta{sv}}" direction="out"/></method></interface></node>"""


class MockWm:
    def __init__(self, conn, windows):
        self.conn, self.windows, self.log = conn, windows, []
        self.n = itertools.count(1)
        self.sessions = {}  # path -> [(stream_path, node_id)]
        sc = Gio.DBusNodeInfo.new_for_xml(SC_XML).interfaces[0]
        self.sess_info = Gio.DBusNodeInfo.new_for_xml(SESSION_XML).interfaces[0]
        self.stream_info = Gio.DBusNodeInfo.new_for_xml(STREAM_XML).interfaces[0]
        intro = Gio.DBusNodeInfo.new_for_xml(INTRO_XML).interfaces[0]
        conn.register_object("/org/gnome/Mutter/ScreenCast", sc, self.on_sc, None, None)
        conn.register_object("/org/gnome/Shell/Introspect", intro, self.on_intro, None, None)
        for name in ("org.gnome.Mutter.ScreenCast", "org.gnome.Shell.Introspect"):
            Gio.bus_own_name_on_connection(conn, name, Gio.BusNameOwnerFlags.NONE, None, None)

    def on_intro(self, c, s, p, i, method, params, inv):
        out = {wid: {"title": GLib.Variant("s", t), "app-id": GLib.Variant("s", a)} for wid, (a, t) in self.windows.items()}
        inv.return_value(GLib.Variant("(a{ta{sv}})", (out,)))

    def on_sc(self, c, s, p, i, method, params, inv):
        path = f"/org/gnome/Mutter/ScreenCast/Session/u{next(self.n)}"
        self.sessions[path] = []
        self.log.append("create")
        self.conn.register_object(path, self.sess_info, self.on_session(path), None, None)
        inv.return_value(GLib.Variant("(o)", (path,)))

    def on_session(self, spath):
        def handler(c, s, p, i, method, params, inv):
            if method in ("RecordMonitor", "RecordWindow"):
                props = params.get_child_value(params.n_children() - 1).unpack()
                cursor = props.get("cursor-mode")
                if method == "RecordMonitor":
                    self.log.append(f"record_monitor {params.get_child_value(0).unpack()} cursor={cursor}")
                    size = (1920, 1080)
                else:
                    self.log.append(f"record_window {props.get('window-id')} cursor={cursor}")
                    size = (1, 1)
                n = next(self.n)
                st = f"/org/gnome/Mutter/ScreenCast/Stream/u{n}"
                params_v = {"position": GLib.Variant("(ii)", (0, 0)), "size": GLib.Variant("(ii)", size)}
                self.conn.register_object(st, self.stream_info, None,
                                          lambda *a, v=params_v: GLib.Variant("a{sv}", v), None)
                self.sessions[spath].append((st, 500 + n))
                inv.return_value(GLib.Variant("(o)", (st,)))
            elif method == "Start":
                self.log.append("start")
                inv.return_value(None)
                for st, node in self.sessions[spath]:
                    self.conn.emit_signal(None, st, "org.gnome.Mutter.ScreenCast.Stream", "PipeWireStreamAdded",
                                          GLib.Variant("(u)", (node,)))
            elif method == "Stop":
                self.log.append("stop")
                inv.return_value(None)
                self.conn.emit_signal(None, spath, "org.gnome.Mutter.ScreenCast.Session", "Closed", None)
        return handler
