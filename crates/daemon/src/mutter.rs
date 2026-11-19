//! Прокси к эмуляции `org.gnome.Mutter.ScreenCast` и `org.gnome.Shell.Introspect` в conchy-wm.
//! Сам поток PipeWire создаёт wm; демон только просит «записать монитор/окно» и забирает node id.

use std::collections::HashMap;

use zbus::{proxy, zvariant::{OwnedObjectPath, OwnedValue, Value}};

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast"
)]
pub trait MutterScreenCast {
    fn create_session(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(interface = "org.gnome.Mutter.ScreenCast.Session", default_service = "org.gnome.Mutter.ScreenCast")]
pub trait MutterSession {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn record_monitor(&self, connector: &str, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
    fn record_window(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
    #[zbus(signal)]
    fn closed(&self) -> zbus::Result<()>;
}

#[proxy(interface = "org.gnome.Mutter.ScreenCast.Stream", default_service = "org.gnome.Mutter.ScreenCast")]
pub trait MutterStream {
    #[zbus(signal)]
    fn pipe_wire_stream_added(&self, node_id: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn parameters(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

#[proxy(
    interface = "org.gnome.Shell.Introspect",
    default_service = "org.gnome.Shell.Introspect",
    default_path = "/org/gnome/Shell/Introspect"
)]
pub trait ShellIntrospect {
    fn get_windows(&self) -> zbus::Result<HashMap<u64, HashMap<String, OwnedValue>>>;
}
