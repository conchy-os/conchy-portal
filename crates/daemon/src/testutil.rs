//! Тестовые хелперы: p2p-соединение zbus без dbus-daemon и без xdp.

use tokio::net::UnixStream;
use zbus::{Connection, Guid, connection::Builder};

/// Возвращает (серверная сторона, клиентская сторона).
pub async fn p2p_pair() -> (Connection, Connection) {
    let (a, b) = UnixStream::pair().unwrap();
    let guid = Guid::generate();
    let (server, client) = tokio::try_join!(
        Builder::unix_stream(a).server(guid).unwrap().p2p().build(),
        Builder::unix_stream(b).p2p().build(),
    )
    .unwrap();
    (server, client)
}

/// Прокси без destination (p2p-соединение не имеет имён).
pub async fn proxy(client: &Connection, path: &str, iface: &'static str) -> zbus::Proxy<'static> {
    zbus::proxy::Builder::new(client)
        .destination("org.conchy.Test")
        .unwrap()
        .path(path.to_owned())
        .unwrap()
        .interface(iface)
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .unwrap()
}
