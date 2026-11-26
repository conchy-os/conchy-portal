//! Прокси к эмуляции `org.gnome.Mutter.ScreenCast` и `org.gnome.Shell.Introspect` в conchy-wm.
//! Сам поток PipeWire создаёт wm; демон только просит «записать монитор/окно» и забирает node id.

use std::collections::HashMap;

use zbus::{
    proxy,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast"
)]
pub trait MutterScreenCast {
    fn create_session(&self, properties: HashMap<&str, Value<'_>>)
    -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast.Session",
    default_service = "org.gnome.Mutter.ScreenCast"
)]
pub trait MutterSession {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn record_monitor(
        &self,
        connector: &str,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
    fn record_window(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
    #[zbus(signal)]
    fn closed(&self) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast.Stream",
    default_service = "org.gnome.Mutter.ScreenCast"
)]
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

/// `org.freedesktop.ScreenSaver` в conchy-wm (idle-inhibit).
#[proxy(
    interface = "org.freedesktop.ScreenSaver",
    default_service = "org.freedesktop.ScreenSaver",
    default_path = "/org/freedesktop/ScreenSaver"
)]
pub trait ScreenSaver {
    fn inhibit(&self, application_name: &str, reason_for_inhibit: &str) -> zbus::Result<u32>;
    fn un_inhibit(&self, cookie: u32) -> zbus::Result<()>;
}

/// Серверы уведомлений (шелл Conchy) — `org.freedesktop.Notifications`.
#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}
