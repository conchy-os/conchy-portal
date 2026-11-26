//! `org.freedesktop.impl.portal.Access`: диалог разрешений (камера, геолокация, скриншот,
//! фоновая работа…). Показывает conchy-shell (`org.conchy.Portal.ShellHost.Access`).
//!
//! Если шелл недоступен, разрешение **не выдаётся** (ответ 2): молчаливое «да» было бы дырой.

use std::collections::HashMap;
use std::time::Instant;

use conchy_portal_proto::{SHELL_HOST_BUS_NAME, access::AccessRequest, response};
use tracing::{info, warn};
use zbus::{
    Connection, ObjectServer, interface,
    zvariant::{ObjectPath, OwnedObjectPath, Value},
};

use crate::hosts::{ShellHostProxy, ensure_host};
use crate::opts::{self, Options, get_bool, get_str};
use crate::request::RequestGuard;

pub const VERSION: u32 = 1;

type Results = HashMap<String, Value<'static>>;

pub struct Access {
    /// В тестах активации нет (p2p-соединение без шины).
    pub activate: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn build_request(
    app_id: String,
    parent_window: String,
    title: String,
    subtitle: String,
    body: String,
    o: &Options,
) -> AccessRequest {
    AccessRequest {
        app_id,
        parent_window,
        title,
        subtitle,
        body,
        modal: get_bool(o, "modal").unwrap_or(true),
        deny_label: get_str(o, "deny_label").unwrap_or_default(),
        grant_label: get_str(o, "grant_label").unwrap_or_default(),
        icon: get_str(o, "icon").unwrap_or_default(),
        choices: opts::get_value(o, "choices")
            .map(opts::parse_choices)
            .unwrap_or_default(),
    }
}

#[interface(name = "org.freedesktop.impl.portal.Access")]
impl Access {
    #[allow(clippy::too_many_arguments)]
    async fn access_dialog(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        subtitle: String,
        body: String,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let started = Instant::now();
        let handle = OwnedObjectPath::from(handle);
        let (_guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => {
                warn!("Access: {e}");
                return (response::OTHER, Results::new());
            }
        };
        let req = build_request(app_id, parent_window, title, subtitle, body, &options);
        info!(%handle, app_id = %req.app_id, title = %req.title, "Access");
        if self.activate
            && let Err(e) = ensure_host(conn, SHELL_HOST_BUS_NAME).await
        {
            warn!("Access: нет хоста conchy-shell, отказываю: {e}");
            return (response::OTHER, Results::new());
        }
        let host = match ShellHostProxy::new(conn).await {
            Ok(h) => h,
            Err(e) => {
                warn!("Access: прокси к хосту: {e}");
                return (response::OTHER, Results::new());
            }
        };
        let id = handle.to_string();
        let out = tokio::select! {
            r = host.access(&id, &req) => match r {
                Ok((code, res)) if code == response::OK => {
                    let choices: Vec<(String, String)> = res.choices.into_iter().map(|c| (c.id, c.value)).collect();
                    let mut m = Results::new();
                    m.insert("choices".into(), Value::from(choices));
                    (code, m)
                }
                Ok((code, _)) => (code, Results::new()),
                Err(e) => {
                    warn!("Access: хост ответил ошибкой: {e}");
                    (response::OTHER, Results::new())
                }
            },
            _ = cancel => {
                let _ = host.cancel(&id).await;
                (response::CANCELLED, Results::new())
            }
        };
        info!(%handle, response = out.0, ms = started.elapsed().as_secs_f64() * 1000.0, "Access готов");
        out
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
    use conchy_portal_proto::access::AccessResult;
    use conchy_portal_proto::files::ChoiceSelection;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Notify;
    use zbus::zvariant::OwnedValue;

    const IFACE: &str = "org.freedesktop.impl.portal.Access";

    #[derive(Default)]
    struct MockShell {
        seen: Arc<Mutex<Vec<(String, AccessRequest)>>>,
        cancelled: Arc<Mutex<Vec<String>>>,
        block: bool,
        stop: Arc<Notify>,
        code: u32,
        fail: bool,
    }

    #[interface(name = "org.conchy.Portal.ShellHost")]
    impl MockShell {
        async fn access(
            &self,
            id: &str,
            req: AccessRequest,
        ) -> zbus::fdo::Result<(u32, AccessResult)> {
            self.seen.lock().unwrap().push((id.to_owned(), req));
            if self.fail {
                return Err(zbus::fdo::Error::Failed("shell crashed".into()));
            }
            if self.block {
                self.stop.notified().await;
                return Ok((1, AccessResult::default()));
            }
            Ok((
                self.code,
                AccessResult {
                    choices: vec![ChoiceSelection {
                        id: "remember".into(),
                        value: "true".into(),
                    }],
                },
            ))
        }
        async fn cancel(&self, id: &str) {
            self.cancelled.lock().unwrap().push(id.to_owned());
            self.stop.notify_one();
        }
    }

    async fn setup(host: MockShell) -> (Connection, Connection) {
        let (server, client) = p2p_pair().await;
        server
            .object_server()
            .at(
                conchy_portal_proto::PORTAL_OBJECT_PATH,
                Access { activate: false },
            )
            .await
            .unwrap();
        client
            .object_server()
            .at("/org/conchy/Portal", host)
            .await
            .unwrap();
        (server, client)
    }

    fn options() -> Options {
        [
            ("grant_label", Value::from("Allow")),
            ("deny_label", Value::from("Deny")),
            ("icon", Value::from("camera-web-symbolic")),
            (
                "choices",
                Value::from(vec![(
                    "remember".to_owned(),
                    "Remember".to_owned(),
                    Vec::<(String, String)>::new(),
                    "false".to_owned(),
                )]),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).unwrap()))
        .collect()
    }

    #[tokio::test]
    async fn grant_returns_choices_and_request_is_parsed() {
        let host = MockShell::default();
        let seen = host.seen.clone();
        let (_s, c) = setup(host).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/x1").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) = p
            .call(
                "AccessDialog",
                &(
                    h,
                    "org.app",
                    "wayland:1",
                    "Allow camera?",
                    "App wants the camera",
                    "body",
                    options(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(code, 0);
        let choices: Vec<(String, String)> = res["choices"].clone().try_into().unwrap();
        assert_eq!(choices, vec![("remember".to_owned(), "true".to_owned())]);
        let req = seen.lock().unwrap()[0].1.clone();
        assert_eq!(
            (
                req.title.as_str(),
                req.subtitle.as_str(),
                req.grant_label.as_str()
            ),
            ("Allow camera?", "App wants the camera", "Allow")
        );
        assert_eq!(req.icon, "camera-web-symbolic");
        assert_eq!(req.choices[0].id, "remember");
    }

    #[tokio::test]
    async fn deny_and_host_failure() {
        let (_s, c) = setup(MockShell {
            code: 1,
            ..Default::default()
        })
        .await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/x2").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) = p
            .call("AccessDialog", &(h, "a", "", "t", "s", "b", Options::new()))
            .await
            .unwrap();
        assert_eq!((code, res.len()), (1, 0));

        // Шелл упал → отказ (2), а не молчаливое разрешение.
        let (_s2, c2) = setup(MockShell {
            fail: true,
            ..Default::default()
        })
        .await;
        let p2 = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/x3").unwrap();
        let (code, _): (u32, HashMap<String, OwnedValue>) = p2
            .call("AccessDialog", &(h, "a", "", "t", "s", "b", Options::new()))
            .await
            .unwrap();
        assert_eq!(code, 2);
    }

    #[tokio::test]
    async fn request_close_cancels_dialog() {
        let host = MockShell {
            block: true,
            ..Default::default()
        };
        let (cancelled, seen) = (host.cancelled.clone(), host.seen.clone());
        let (_s, c) = setup(host).await;
        let c2 = c.clone();
        let call = tokio::spawn(async move {
            let p = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
            let h = ObjectPath::try_from("/r/x4").unwrap();
            p.call::<_, _, (u32, HashMap<String, OwnedValue>)>(
                "AccessDialog",
                &(h, "a", "", "t", "s", "b", Options::new()),
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
        let req = proxy(&c, "/r/x4", "org.freedesktop.impl.portal.Request").await;
        req.call_method("Close", &()).await.unwrap();
        let (code, _) = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code, 1);
        assert_eq!(cancelled.lock().unwrap().as_slice(), ["/r/x4"]);
    }
}
