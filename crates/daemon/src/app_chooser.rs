//! `org.freedesktop.impl.portal.AppChooser`: «Открыть с помощью». Диалог показывает conchy-files
//! (`FilesHost.ChooseApp`), демон разбирает запрос и собирает ответ.

use std::collections::HashMap;
use std::time::Instant;

use conchy_portal_proto::{FILES_HOST_BUS_NAME, apps::AppChooseRequest, response};
use tracing::{info, warn};
use zbus::{
    Connection, ObjectServer, interface,
    zvariant::{ObjectPath, OwnedObjectPath, Value},
};

use crate::hosts::{FilesHostProxy, ensure_host};
use crate::opts::{Options, get_bool, get_str};
use crate::request::RequestGuard;

pub const VERSION: u32 = 2;

type Results = HashMap<String, Value<'static>>;

pub struct AppChooser {
    /// В тестах активации нет (p2p-соединение без шины).
    pub activate: bool,
}

pub fn build_request(app_id: String, parent_window: String, choices: Vec<String>, o: &Options) -> AppChooseRequest {
    AppChooseRequest {
        app_id,
        parent_window,
        choices,
        last_choice: get_str(o, "last_choice").unwrap_or_default(),
        modal: get_bool(o, "modal").unwrap_or(true),
        content_type: get_str(o, "content_type").unwrap_or_default(),
        uri: get_str(o, "uri").unwrap_or_default(),
        filename: get_str(o, "filename").unwrap_or_default(),
        activation_token: get_str(o, "activation_token").unwrap_or_default(),
    }
}

#[interface(name = "org.freedesktop.impl.portal.AppChooser")]
impl AppChooser {
    async fn choose_application(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        choices: Vec<String>,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let started = Instant::now();
        let handle = OwnedObjectPath::from(handle);
        let (_guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => {
                warn!("AppChooser: {e}");
                return (response::OTHER, Results::new());
            }
        };
        let req = build_request(app_id, parent_window, choices, &options);
        info!(%handle, app_id = %req.app_id, content_type = %req.content_type, choices = req.choices.len(), "AppChooser");
        if self.activate {
            if let Err(e) = ensure_host(conn, FILES_HOST_BUS_NAME).await {
                warn!("AppChooser: нет хоста conchy-files: {e}");
                return (response::OTHER, Results::new());
            }
        }
        let host = match FilesHostProxy::new(conn).await {
            Ok(h) => h,
            Err(e) => {
                warn!("AppChooser: прокси к хосту: {e}");
                return (response::OTHER, Results::new());
            }
        };
        let id = handle.to_string();
        let out = tokio::select! {
            r = host.choose_app(&id, &req) => match r {
                Ok((code, res)) if code == response::OK && !res.choice.is_empty() => {
                    let mut m = Results::new();
                    m.insert("choice".into(), Value::from(res.choice));
                    if !res.activation_token.is_empty() {
                        m.insert("activation_token".into(), Value::from(res.activation_token));
                    }
                    (code, m)
                }
                Ok((code, _)) if code != response::OK => (code, Results::new()),
                Ok(_) => (response::CANCELLED, Results::new()),
                Err(e) => {
                    warn!("AppChooser: хост ответил ошибкой: {e}");
                    (response::OTHER, Results::new())
                }
            },
            _ = cancel => {
                let _ = host.cancel(&id).await;
                (response::CANCELLED, Results::new())
            }
        };
        info!(%handle, response = out.0, ms = started.elapsed().as_secs_f64() * 1000.0, "AppChooser готов");
        out
    }

    /// Обновить список в открытом диалоге (приложения установились/удалились).
    async fn update_choices(
        &self,
        handle: ObjectPath<'_>,
        choices: Vec<String>,
        #[zbus(connection)] conn: &Connection,
    ) {
        match FilesHostProxy::new(conn).await {
            Ok(host) => {
                if let Err(e) = host.update_choices(handle.as_str(), &choices).await {
                    warn!("UpdateChoices: {e}");
                }
            }
            Err(e) => warn!("UpdateChoices: прокси к хосту: {e}"),
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{p2p_pair, proxy};
    use conchy_portal_proto::apps::AppChooseResult;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Notify;
    use zbus::zvariant::OwnedValue;

    const IFACE: &str = "org.freedesktop.impl.portal.AppChooser";

    #[derive(Default)]
    struct MockHost {
        seen: Arc<Mutex<Vec<(String, AppChooseRequest)>>>,
        updates: Arc<Mutex<Vec<(String, Vec<String>)>>>,
        cancelled: Arc<Mutex<Vec<String>>>,
        block: bool,
        stop: Arc<Notify>,
        result: AppChooseResult,
        code: u32,
    }

    #[interface(name = "org.conchy.Portal.FilesHost")]
    impl MockHost {
        async fn choose_app(&self, id: &str, req: AppChooseRequest) -> (u32, AppChooseResult) {
            self.seen.lock().unwrap().push((id.to_owned(), req));
            if self.block {
                self.stop.notified().await;
                return (1, AppChooseResult::default());
            }
            (self.code, self.result.clone())
        }
        async fn update_choices(&self, id: &str, choices: Vec<String>) {
            self.updates.lock().unwrap().push((id.to_owned(), choices));
        }
        async fn cancel(&self, id: &str) {
            self.cancelled.lock().unwrap().push(id.to_owned());
            self.stop.notify_one();
        }
    }

    async fn setup(host: MockHost) -> (Connection, Connection) {
        let (server, client) = p2p_pair().await;
        server
            .object_server()
            .at(conchy_portal_proto::PORTAL_OBJECT_PATH, AppChooser { activate: false })
            .await
            .unwrap();
        client.object_server().at("/org/conchy/Portal", host).await.unwrap();
        (server, client)
    }

    fn opts() -> Options {
        [
            ("last_choice", Value::from("org.b")),
            ("content_type", Value::from("image/png")),
            ("filename", Value::from("a.png")),
            ("modal", Value::from(false)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).unwrap()))
        .collect()
    }

    #[tokio::test]
    async fn choose_returns_choice_and_parses_options() {
        let host = MockHost {
            result: AppChooseResult { choice: "org.b".into(), activation_token: "tok".into() },
            ..Default::default()
        };
        let seen = host.seen.clone();
        let (_s, c) = setup(host).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/a1").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) = p
            .call("ChooseApplication", &(h, "org.app", "", vec!["org.a", "org.b"], opts()))
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(String::try_from(res["choice"].clone()).unwrap(), "org.b");
        assert_eq!(String::try_from(res["activation_token"].clone()).unwrap(), "tok");
        let req = seen.lock().unwrap()[0].1.clone();
        assert_eq!(req.choices, ["org.a", "org.b"]);
        assert_eq!(req.last_choice, "org.b");
        assert_eq!(req.content_type, "image/png");
        assert!(!req.modal);
    }

    #[tokio::test]
    async fn empty_choice_is_cancel_and_cancel_code_passes() {
        let (_s, c) = setup(MockHost::default()).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/a2").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) =
            p.call("ChooseApplication", &(h, "a", "", Vec::<String>::new(), Options::new())).await.unwrap();
        assert_eq!((code, res.len()), (1, 0));
        let (_s2, c2) = setup(MockHost { code: 1, ..Default::default() }).await;
        let p2 = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/a3").unwrap();
        let (code, _): (u32, HashMap<String, OwnedValue>) =
            p2.call("ChooseApplication", &(h, "a", "", Vec::<String>::new(), Options::new())).await.unwrap();
        assert_eq!(code, 1);
    }

    #[tokio::test]
    async fn update_choices_forwarded_and_close_cancels() {
        let host = MockHost { block: true, ..Default::default() };
        let (updates, cancelled, seen) = (host.updates.clone(), host.cancelled.clone(), host.seen.clone());
        let (_s, c) = setup(host).await;
        let c2 = c.clone();
        let call = tokio::spawn(async move {
            let p = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
            let h = ObjectPath::try_from("/r/a4").unwrap();
            p.call::<_, _, (u32, HashMap<String, OwnedValue>)>(
                "ChooseApplication",
                &(h, "a", "", vec!["x"], Options::new()),
            )
            .await
            .unwrap()
        });
        for _ in 0..100 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/a4").unwrap();
        p.call_method("UpdateChoices", &(h, vec!["x", "y"])).await.unwrap();
        assert_eq!(updates.lock().unwrap()[0], ("/r/a4".to_owned(), vec!["x".to_owned(), "y".to_owned()]));
        let req = proxy(&c, "/r/a4", "org.freedesktop.impl.portal.Request").await;
        req.call_method("Close", &()).await.unwrap();
        let (code, _) = tokio::time::timeout(Duration::from_secs(2), call).await.unwrap().unwrap();
        assert_eq!(code, 1);
        assert_eq!(cancelled.lock().unwrap().as_slice(), ["/r/a4"]);
    }
}
