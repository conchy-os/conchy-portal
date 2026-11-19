//! Протокол «демон портала ↔ хост UI».
//!
//! Только типы и константы: крейт не тянет ни `zbus`, ни `tokio`, ни `gpui`, поэтому его без
//! конфликтов версий подключают и демон, и `conchy-files`, и `conchy-shell`.

/// Имя демона на сессионной шине (то, что ищет `xdg-desktop-portal` по `.portal`-файлу).
pub const DAEMON_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.conchy";
/// Единый путь всех impl-интерфейсов портала.
pub const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

/// Общий путь объекта хоста UI.
pub const HOST_OBJECT_PATH: &str = "/org/conchy/Portal";
/// Имя хоста файлового менеджера (FileChooser, AppChooser).
pub const FILES_HOST_BUS_NAME: &str = "org.conchy.Portal.FilesHost";
/// Имя хоста шелла (Access, пикер ScreenCast).
pub const SHELL_HOST_BUS_NAME: &str = "org.conchy.Portal.ShellHost";

/// Коды ответа impl-порталов (`response`).
pub mod response {
    /// Успех.
    pub const OK: u32 = 0;
    /// Пользователь отменил.
    pub const CANCELLED: u32 = 1;
    /// Другая ошибка или прервано.
    pub const OTHER: u32 = 2;
}
