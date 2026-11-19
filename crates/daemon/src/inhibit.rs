//! `org.freedesktop.impl.portal.Inhibit`: запрет простоя на время жизни `Request`.
//!
//! Фронтенд вызывает `Inhibit` и держит `handle`; снять блокировку — значит закрыть запрос
//! (`Request.Close`). Метод возвращается сразу, а блокировку держит фоновая задача:
//! при `Close` она зовёт `UnInhibit` у conchy-wm. Реально поддерживаем флаг «простой» (8);
//! выход/переключение пользователя/suspend (1, 2, 4) принимаем, но ничего не блокируем.

use tracing::{info, warn};
use zbus::{
    Connection, ObjectServer, interface,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use crate::mutter::ScreenSaverProxy;
use crate::opts::{Options, get_str};
use crate::request::RequestGuard;
use crate::session::SessionGuard;

pub const VERSION: u32 = 3;
pub const FLAG_IDLE: u32 = 8;

pub struct Inhibit;

#[interface(name = "org.freedesktop.impl.portal.Inhibit")]
impl Inhibit {
    async fn inhibit(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        window: String,
        flags: u32,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) {
        let handle = OwnedObjectPath::from(handle);
        let (guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => return warn!("Inhibit: {e}"),
        };
        info!(%handle, app_id, window, flags, "Inhibit");
        let conn = conn.clone();
        let reason = get_str(&options, "reason").unwrap_or_default();
        tokio::spawn(async move {
            let _guard = guard; // объект Request живёт, пока держится блокировка
            let held = if flags & FLAG_IDLE != 0 {
                match ScreenSaverProxy::new(&conn).await {
                    Ok(p) => match p.inhibit(if app_id.is_empty() { "portal" } else { &app_id }, &reason).await {
                        Ok(cookie) => Some((p, cookie)),
                        Err(e) => {
                            warn!("ScreenSaver.Inhibit: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        warn!("нет ScreenSaver: {e}");
                        None
                    }
                }
            } else {
                None
            };
            let _ = cancel.await; // Request.Close (или падение демона)
            if let Some((p, cookie)) = held {
                if let Err(e) = p.un_inhibit(cookie).await {
                    warn!("ScreenSaver.UnInhibit: {e}");
                }
            }
        });
    }

    /// Монитор состояния сессии (экран заблокирован, скоро выход). Состояния пока не
    /// отслеживаем: сессию экспортируем, `StateChanged` не шлём.
    async fn create_monitor(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        _app_id: String,
        _window: String,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> u32 {
        let (_g, _c) = match RequestGuard::new(server, OwnedObjectPath::from(handle)).await {
            Ok(g) => g,
            Err(e) => {
                warn!("CreateMonitor: {e}");
                return conchy_portal_proto::response::OTHER;
            }
        };
        match SessionGuard::new(server, OwnedObjectPath::from(session_handle), 1).await {
            Ok(session) => {
                tokio::spawn(async move { session.wait_closed().await });
                conchy_portal_proto::response::OK
            }
            Err(e) => {
                warn!("CreateMonitor: {e}");
                conchy_portal_proto::response::OTHER
            }
        }
    }

    async fn query_end_response(&self, _session_handle: ObjectPath<'_>) {}

    #[zbus(signal)]
    async fn state_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        state: std::collections::HashMap<String, zbus::zvariant::Value<'_>>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{p2p_pair, proxy};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct MockSaver {
        log: Arc<Mutex<Vec<String>>>,
    }

    #[interface(name = "org.freedesktop.ScreenSaver")]
    impl MockSaver {
        async fn inhibit(&self, app: &str, reason: &str) -> u32 {
            self.log.lock().unwrap().push(format!("inhibit {app} {reason}"));
            77
        }
        async fn un_inhibit(&self, cookie: u32) {
            self.log.lock().unwrap().push(format!("uninhibit {cookie}"));
        }
    }

    async fn wait_len(log: &Arc<Mutex<Vec<String>>>, n: usize) {
        for _ in 0..100 {
            if log.lock().unwrap().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn idle_inhibit_lives_until_request_close() {
        let (server, client) = p2p_pair().await;
        server.object_server().at(conchy_portal_proto::PORTAL_OBJECT_PATH, Inhibit).await.unwrap();
        let saver = MockSaver::default();
        let log = saver.log.clone();
        client.object_server().at("/org/freedesktop/ScreenSaver", saver).await.unwrap();

        let p = proxy(&client, conchy_portal_proto::PORTAL_OBJECT_PATH, "org.freedesktop.impl.portal.Inhibit").await;
        let h = ObjectPath::try_from("/r/i1").unwrap();
        let opts: Options = [("reason".to_string(), zbus::zvariant::OwnedValue::try_from(zbus::zvariant::Value::from("Playing video")).unwrap())]
            .into_iter()
            .collect();
        p.call_method("Inhibit", &(h, "org.app", "", FLAG_IDLE | 4, opts)).await.unwrap();
        wait_len(&log, 1).await;
        assert_eq!(log.lock().unwrap()[0], "inhibit org.app Playing video");

        // Пока запрос не закрыт, блокировка держится и объект Request экспортирован.
        let req = proxy(&client, "/r/i1", "org.freedesktop.impl.portal.Request").await;
        req.call_method("Close", &()).await.unwrap();
        wait_len(&log, 2).await;
        assert_eq!(log.lock().unwrap()[1], "uninhibit 77");
    }

    #[tokio::test]
    async fn non_idle_flags_do_not_touch_screensaver() {
        let (server, client) = p2p_pair().await;
        server.object_server().at(conchy_portal_proto::PORTAL_OBJECT_PATH, Inhibit).await.unwrap();
        let saver = MockSaver::default();
        let log = saver.log.clone();
        client.object_server().at("/org/freedesktop/ScreenSaver", saver).await.unwrap();
        let p = proxy(&client, conchy_portal_proto::PORTAL_OBJECT_PATH, "org.freedesktop.impl.portal.Inhibit").await;
        let h = ObjectPath::try_from("/r/i2").unwrap();
        p.call_method("Inhibit", &(h, "a", "", 1u32 | 4, Options::new())).await.unwrap();
        let req = proxy(&client, "/r/i2", "org.freedesktop.impl.portal.Request").await;
        req.call_method("Close", &()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_monitor_exports_session() {
        let (server, client) = p2p_pair().await;
        server.object_server().at(conchy_portal_proto::PORTAL_OBJECT_PATH, Inhibit).await.unwrap();
        let p = proxy(&client, conchy_portal_proto::PORTAL_OBJECT_PATH, "org.freedesktop.impl.portal.Inhibit").await;
        let code: u32 = p
            .call("CreateMonitor", &(ObjectPath::try_from("/r/m").unwrap(), ObjectPath::try_from("/s/m").unwrap(), "a", ""))
            .await
            .unwrap();
        assert_eq!(code, 0);
        let s = proxy(&client, "/s/m", "org.freedesktop.impl.portal.Session").await;
        s.call_method("Close", &()).await.unwrap();
    }
}
