//! Объект `org.freedesktop.impl.portal.Request`.
//!
//! Пока метод портала выполняется, на `handle` обязан висеть объект с методом `Close()`.
//! [`RequestGuard`] экспортирует его при создании и снимает при `Drop`.

use std::sync::Mutex;

use tokio::sync::oneshot;
use zbus::{ObjectServer, interface, zvariant::OwnedObjectPath};

/// Сторона объекта шины: `Close()` будит ожидающего [`CancelRx`].
pub struct Request {
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    async fn close(&self) {
        let tx = self.cancel.lock().map(|mut g| g.take()).unwrap_or(None);
        if let Some(tx) = tx {
            let _ = tx.send(());
        }
    }
}

/// Резолвится, когда фронтенд вызвал `Request.Close`. Если guard снят без отмены — `Err`.
pub type CancelRx = oneshot::Receiver<()>;

/// RAII: пока жив — объект `Request` экспортирован на `handle`.
pub struct RequestGuard {
    server: ObjectServer,
    path: OwnedObjectPath,
}

impl RequestGuard {
    /// Экспортирует объект на `handle`. `Err`, если путь уже занят.
    pub async fn new(
        server: &ObjectServer,
        handle: OwnedObjectPath,
    ) -> zbus::Result<(Self, CancelRx)> {
        let (tx, rx) = oneshot::channel();
        let req = Request {
            cancel: Mutex::new(Some(tx)),
        };
        if !server.at(&handle, req).await? {
            return Err(zbus::Error::Failure(format!(
                "request handle {handle} уже занят"
            )));
        }
        Ok((
            Self {
                server: server.clone(),
                path: handle,
            },
            rx,
        ))
    }

}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let server = self.server.clone();
        let path = self.path.clone();
        // Drop синхронный, а снятие экспорта асинхронное.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = server.remove::<Request, _>(&path).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::p2p_pair;

    #[tokio::test]
    async fn export_close_and_unexport() {
        let (server, client) = p2p_pair().await;
        let handle = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/t/1").unwrap();

        let (guard, cancel) = RequestGuard::new(server.object_server(), handle.clone())
            .await
            .unwrap();
        // Повторный экспорт на тот же путь — ошибка.
        assert!(RequestGuard::new(server.object_server(), handle.clone()).await.is_err());

        let proxy = crate::testutil::proxy(&client, handle.as_str(), "org.freedesktop.impl.portal.Request").await;
        proxy.call_method("Close", &()).await.unwrap();
        assert!(cancel.await.is_ok(), "Close должен будить cancel-канал");

        drop(guard);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // После Drop объект снят.
        assert!(proxy.call_method("Close", &()).await.is_err());
    }

    #[tokio::test]
    async fn drop_without_close_cancels_nothing() {
        let (server, _client) = p2p_pair().await;
        let handle = OwnedObjectPath::try_from("/r/2").unwrap();
        let (guard, cancel) = RequestGuard::new(server.object_server(), handle).await.unwrap();
        drop(guard);
        // Sender живёт внутри снятого объекта и дропается — получатель видит Err.
        let r = tokio::time::timeout(std::time::Duration::from_secs(1), cancel).await;
        assert!(matches!(r, Ok(Err(_))));
    }
}
