//! Прокси к хостам UI и их D-Bus-активация.

use std::time::Duration;

use conchy_portal_proto::apps::{AppChooseRequest, AppChooseResult};
use conchy_portal_proto::files::{FilePickRequest, FilePickResult};
use tracing::{debug, warn};
use zbus::{Connection, fdo, proxy};

/// Сколько ждём запуска хоста по активации.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(2);

/// `org.conchy.Portal.FilesHost` (в conchy-files).
///
/// У вызова **нет** клиентского таймаута: пользователь может думать минуты. Падение хоста
/// приходит ошибкой (`NoReply`/`ServiceUnknown`) — демон отвечает порталу кодом 2.
#[proxy(
    interface = "org.conchy.Portal.FilesHost",
    default_service = "org.conchy.Portal.FilesHost",
    default_path = "/org/conchy/Portal"
)]
pub trait FilesHost {
    fn pick(&self, id: &str, req: &FilePickRequest) -> zbus::Result<(u32, FilePickResult)>;
    fn choose_app(&self, id: &str, req: &AppChooseRequest) -> zbus::Result<(u32, AppChooseResult)>;
    fn update_choices(&self, id: &str, choices: &[String]) -> zbus::Result<()>;
    fn cancel(&self, id: &str) -> zbus::Result<()>;
}

/// Поднимает хост через D-Bus-активацию, если имя ещё не занято. На p2p-соединении (тесты)
/// активации нет и функция ничего не делает.
pub async fn ensure_host(conn: &Connection, name: &'static str) -> Result<(), String> {
    if conn.unique_name().is_none() {
        return Ok(());
    }
    let bus = fdo::DBusProxy::new(conn).await.map_err(|e| e.to_string())?;
    let bus_name = zbus::names::BusName::try_from(name).map_err(|e| e.to_string())?;
    if bus.name_has_owner(bus_name).await.unwrap_or(false) {
        return Ok(());
    }
    debug!(name, "хост не запущен, запускаю по активации");
    let wk = zbus::names::WellKnownName::try_from(name).map_err(|e| e.to_string())?;
    match tokio::time::timeout(ACTIVATION_TIMEOUT, bus.start_service_by_name(wk, 0)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => {
            warn!(name, "активация хоста не удалась: {e}");
            Err(e.to_string())
        }
        Err(_) => Err(format!("{name} не запустился за {ACTIVATION_TIMEOUT:?}")),
    }
}
