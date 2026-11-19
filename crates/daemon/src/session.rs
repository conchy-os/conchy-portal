//! Объект `org.freedesktop.impl.portal.Session` (ScreenCast и др.).
//!
//! Живёт до закрытия сессии: `Close()` от фронтенда или закрытие со стороны бэкенда
//! ([`SessionGuard::close_by_backend`] → сигнал `Closed`).

use std::sync::Arc;

use tokio::sync::Notify;
use zbus::{ObjectServer, interface, object_server::SignalEmitter, zvariant::OwnedObjectPath};

pub struct Session {
    version: u32,
    closed: Arc<Notify>,
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(&self) {
        // notify_one хранит permit — ждущий увидит событие, даже если придёт позже.
        self.closed.notify_one();
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.version
    }
}

/// Владелец сессии. `Drop` снимает объект с шины.
pub struct SessionGuard {
    server: ObjectServer,
    path: OwnedObjectPath,
    closed: Arc<Notify>,
}

impl SessionGuard {
    pub async fn new(
        server: &ObjectServer,
        path: OwnedObjectPath,
        version: u32,
    ) -> zbus::Result<Self> {
        let closed = Arc::new(Notify::new());
        let s = Session {
            version,
            closed: closed.clone(),
        };
        if !server.at(&path, s).await? {
            return Err(zbus::Error::Failure(format!("session {path} уже существует")));
        }
        Ok(Self {
            server: server.clone(),
            path,
            closed,
        })
    }

    /// Ждёт `Session.Close()` от фронтенда.
    pub async fn wait_closed(&self) {
        self.closed.notified().await;
    }

    /// Бэкенд закрывает сессию сам: шлёт `Closed` и снимает объект.
    pub async fn close_by_backend(self) -> zbus::Result<()> {
        let iface = self.server.interface::<_, Session>(&self.path).await?;
        Session::closed(iface.signal_emitter()).await
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let server = self.server.clone();
        let path = self.path.clone();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = server.remove::<Session, _>(&path).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::p2p_pair;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn close_wakes_waiter_and_unexports() {
        let (server, client) = p2p_pair().await;
        let path = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/t/1").unwrap();
        let guard = SessionGuard::new(server.object_server(), path.clone(), 5).await.unwrap();

        let proxy = crate::testutil::proxy(&client, path.as_str(), "org.freedesktop.impl.portal.Session").await;
        let version: u32 = proxy.get_property("version").await.unwrap();
        assert_eq!(version, 5);

        proxy.call_method("Close", &()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), guard.wait_closed())
            .await
            .expect("Close должен будить wait_closed");

        drop(guard);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(proxy.call_method("Close", &()).await.is_err());
    }

    #[tokio::test]
    async fn backend_close_emits_signal() {
        let (server, client) = p2p_pair().await;
        let path = OwnedObjectPath::try_from("/s/2").unwrap();
        let guard = SessionGuard::new(server.object_server(), path.clone(), 1).await.unwrap();

        let proxy = crate::testutil::proxy(&client, path.as_str(), "org.freedesktop.impl.portal.Session").await;
        let mut sig = proxy.receive_signal("Closed").await.unwrap();
        guard.close_by_backend().await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), sig.next()).await;
        assert!(matches!(got, Ok(Some(_))));
    }
}
