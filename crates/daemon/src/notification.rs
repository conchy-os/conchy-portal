//! `org.freedesktop.impl.portal.Notification`: тонкий мост к `org.freedesktop.Notifications`
//! (сервер уведомлений в шелле). Фронтенд xdp шлёт `AddNotification(app_id, id, {…})`, мы
//! превращаем это в `Notify` и помним соответствие `(app_id, id)` ↔ id сервера уведомлений:
//! по нему обновляем/снимаем уведомление и пробрасываем нажатые действия обратно сигналом
//! `ActionInvoked`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use tracing::{debug, warn};
use zbus::{
    Connection, interface,
    object_server::{InterfaceRef, SignalEmitter},
    zvariant::{OwnedValue, Value},
};

use crate::mutter::NotificationsProxy;
use crate::opts::Options;

pub const VERSION: u32 = 2;
/// Ключ действия «по нажатию на уведомление» в `org.freedesktop.Notifications`.
const DEFAULT_ACTION: &str = "default";

#[derive(Default)]
struct State {
    /// `(app_id, id)` → id у сервера уведомлений.
    by_key: HashMap<(String, String), u32>,
    /// id у сервера → `(app_id, id, действия)`: ключ действия у сервера → (действие портала, цель).
    by_fdo: HashMap<u32, Sent>,
}

struct Sent {
    app_id: String,
    id: String,
    actions: HashMap<String, (String, Option<OwnedValue>)>,
}

#[derive(Clone, Default)]
pub struct Notification {
    state: Arc<Mutex<State>>,
}

/// Результат разбора `a{sv}` портала в аргументы `Notify`.
#[derive(Debug, PartialEq)]
pub struct Parsed {
    pub summary: String,
    pub body: String,
    pub icon: String,
    /// `urgency`: 0 низкая, 1 обычная, 2 высокая.
    pub urgency: u8,
    /// Плоский список `[ключ, подпись, …]` для `Notify`.
    pub actions: Vec<(String, String)>,
}

fn text(o: &Options, k: &str) -> String {
    o.get(k)
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_default()
}

/// Первое имя темы из иконки `(sv)` вида `("themed", ["name", …])`; файловые и байтовые
/// иконки не поддерживаются (v2 портала их не присылает как bytes).
fn icon_name(o: &Options) -> String {
    let Some(v) = o.get("icon") else {
        return String::new();
    };
    let Value::Structure(s) = &**v else {
        return String::new();
    };
    let [Value::Str(kind), Value::Value(inner)] = s.fields() else {
        return String::new();
    };
    if kind.as_str() != "themed" {
        return String::new();
    }
    match &**inner {
        Value::Array(a) => a
            .iter()
            .find_map(|x| {
                if let Value::Str(s) = x {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub fn parse(o: &Options) -> Parsed {
    let urgency = match text(o, "priority").as_str() {
        "low" => 0,
        "high" | "urgent" => 2,
        _ => 1,
    };
    let mut actions = Vec::new();
    if !text(o, "default-action").is_empty() {
        actions.push((DEFAULT_ACTION.to_string(), String::new()));
    }
    if let Some(buttons) = o
        .get("buttons")
        .and_then(|v| Vec::<HashMap<String, OwnedValue>>::try_from(v.clone()).ok())
    {
        for (i, b) in buttons.iter().enumerate() {
            let (label, action) = (text(b, "label"), text(b, "action"));
            if !action.is_empty() {
                // Ключ у сервера уникален по позиции: одно и то же действие может быть у двух кнопок.
                actions.push((format!("btn{i}:{action}"), label));
            }
        }
    }
    let body = {
        let b = text(o, "body");
        if b.is_empty() {
            text(o, "markup-body")
        } else {
            b
        }
    };
    Parsed {
        summary: text(o, "title"),
        body,
        icon: icon_name(o),
        urgency,
        actions,
    }
}

#[interface(name = "org.freedesktop.impl.portal.Notification")]
impl Notification {
    async fn add_notification(
        &self,
        app_id: String,
        id: String,
        notification: Options,
        #[zbus(connection)] conn: &Connection,
    ) {
        let p = parse(&notification);
        debug!(app_id, id, title = %p.summary, "AddNotification");
        let proxy = match NotificationsProxy::new(conn).await {
            Ok(p) => p,
            Err(e) => return warn!("нет сервера уведомлений: {e}"),
        };
        let replaces = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.by_key.get(&(app_id.clone(), id.clone())).copied())
            .unwrap_or(0);
        let flat: Vec<&str> = p
            .actions
            .iter()
            .flat_map(|(k, l)| [k.as_str(), l.as_str()])
            .collect();
        let hints = HashMap::from([("urgency", Value::from(p.urgency))]);
        match proxy
            .notify(
                &app_id, replaces, &p.icon, &p.summary, &p.body, &flat, hints, -1,
            )
            .await
        {
            Ok(fdo_id) => {
                let default_target = notification
                    .get("default-action")
                    .map(|_| text(&notification, "default-action"));
                let mut actions: HashMap<String, (String, Option<OwnedValue>)> = HashMap::new();
                if let Some(a) = default_target {
                    actions.insert(
                        DEFAULT_ACTION.into(),
                        (
                            a,
                            notification
                                .get("default-action-target")
                                .and_then(|v| v.try_clone().ok()),
                        ),
                    );
                }
                for (k, _) in &p.actions {
                    if let Some((_, action)) = k.strip_prefix("btn").and_then(|r| r.split_once(':'))
                    {
                        actions.insert(k.clone(), (action.to_string(), None));
                    }
                }
                if let Ok(mut s) = self.state.lock() {
                    s.by_key.insert((app_id.clone(), id.clone()), fdo_id);
                    s.by_fdo.insert(
                        fdo_id,
                        Sent {
                            app_id,
                            id,
                            actions,
                        },
                    );
                }
            }
            Err(e) => warn!("Notify: {e}"),
        }
    }

    async fn remove_notification(
        &self,
        app_id: String,
        id: String,
        #[zbus(connection)] conn: &Connection,
    ) {
        let fdo_id = self.state.lock().ok().and_then(|mut s| {
            let k = s.by_key.remove(&(app_id, id))?;
            s.by_fdo.remove(&k);
            Some(k)
        });
        if let Some(fdo_id) = fdo_id
            && let Ok(p) = NotificationsProxy::new(conn).await
        {
            let _ = p.close_notification(fdo_id).await;
        }
    }

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        app_id: &str,
        id: &str,
        action: &str,
        parameter: Vec<Value<'_>>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "SupportedOptions")]
    fn supported_options(&self) -> HashMap<String, Value<'static>> {
        HashMap::new()
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

impl Notification {
    /// Пробрасывает нажатия действий и закрытия из сервера уведомлений в портал.
    pub async fn forward_signals(
        self,
        conn: Connection,
        iface: InterfaceRef<Notification>,
    ) -> zbus::Result<()> {
        let proxy = NotificationsProxy::new(&conn).await?;
        let mut invoked = proxy.receive_action_invoked().await?;
        let mut closed = proxy.receive_notification_closed().await?;
        loop {
            tokio::select! {
                Some(sig) = invoked.next() => {
                    let Ok(args) = sig.args() else { continue };
                    let hit = self.state.lock().ok().and_then(|s| {
                        let sent = s.by_fdo.get(&args.id)?;
                        let (action, target) = sent.actions.get(&args.action_key)?;
                        Some((sent.app_id.clone(), sent.id.clone(), action.clone(), target.as_ref().and_then(|t| t.try_clone().ok())))
                    });
                    if let Some((app_id, id, action, target)) = hit {
                        let param: Vec<Value<'_>> = target.map(|t| vec![Value::from(t)]).unwrap_or_default();
                        let action = if args.action_key == DEFAULT_ACTION { DEFAULT_ACTION.to_string() } else { action };
                        let _ = Notification::action_invoked(iface.signal_emitter(), &app_id, &id, &action, param).await;
                    }
                }
                Some(sig) = closed.next() => {
                    if let Ok(args) = sig.args()
                        && let Ok(mut s) = self.state.lock()
                            && let Some(sent) = s.by_fdo.remove(&args.id) {
                                s.by_key.remove(&(sent.app_id, sent.id));
                            }
                }
                else => return Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{p2p_pair, proxy};
    use std::time::Duration;

    type Calls = Arc<Mutex<Vec<String>>>;

    #[derive(Clone, Default)]
    struct MockServer {
        calls: Calls,
        next: Arc<Mutex<u32>>,
    }

    #[interface(name = "org.freedesktop.Notifications")]
    impl MockServer {
        #[allow(clippy::too_many_arguments)]
        async fn notify(
            &self,
            app: &str,
            replaces: u32,
            icon: &str,
            summary: &str,
            body: &str,
            actions: Vec<String>,
            hints: HashMap<String, Value<'_>>,
            _timeout: i32,
        ) -> u32 {
            let urg = hints
                .get("urgency")
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(9);
            self.calls.lock().unwrap().push(format!(
                "notify {app} r={replaces} icon={icon} {summary}|{body} u={urg} a={actions:?}"
            ));
            if replaces != 0 {
                return replaces;
            }
            let mut n = self.next.lock().unwrap();
            *n += 1;
            100 + *n
        }
        async fn close_notification(&self, id: u32) {
            self.calls.lock().unwrap().push(format!("close {id}"));
        }
        #[zbus(signal)]
        async fn action_invoked(ctxt: &SignalEmitter<'_>, id: u32, key: &str) -> zbus::Result<()>;
    }

    fn opts(items: Vec<(&str, Value<'static>)>) -> Options {
        items
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).unwrap()))
            .collect()
    }

    #[test]
    fn parses_portal_notification() {
        let button = HashMap::from([
            ("label".to_string(), Value::from("Reply")),
            ("action".to_string(), Value::from("reply")),
        ]);
        let o = opts(vec![
            ("title", Value::from("New mail")),
            ("body", Value::from("Hello")),
            ("priority", Value::from("high")),
            ("default-action", Value::from("open")),
            (
                "icon",
                Value::from((
                    "themed".to_string(),
                    Value::new(vec!["mail-unread".to_string(), "mail".to_string()]),
                )),
            ),
            ("buttons", Value::from(vec![button])),
        ]);
        let p = parse(&o);
        assert_eq!(p.summary, "New mail");
        assert_eq!(p.body, "Hello");
        assert_eq!(p.icon, "mail-unread");
        assert_eq!(p.urgency, 2);
        assert_eq!(
            p.actions,
            vec![
                ("default".to_string(), String::new()),
                ("btn0:reply".to_string(), "Reply".to_string())
            ]
        );
        assert_eq!(parse(&Options::new()).urgency, 1);
        assert_eq!(
            parse(&opts(vec![("priority", Value::from("low"))])).urgency,
            0
        );
        assert_eq!(
            parse(&opts(vec![("markup-body", Value::from("<b>x</b>"))])).body,
            "<b>x</b>"
        );
    }

    #[tokio::test]
    async fn add_replace_remove_and_action_forwarding() {
        let (server, client) = p2p_pair().await;
        let n = Notification::default();
        server
            .object_server()
            .at(conchy_portal_proto::PORTAL_OBJECT_PATH, n.clone())
            .await
            .unwrap();
        let mock = MockServer::default();
        let calls = mock.calls.clone();
        client
            .object_server()
            .at("/org/freedesktop/Notifications", mock)
            .await
            .unwrap();
        let iface = server
            .object_server()
            .interface::<_, Notification>(conchy_portal_proto::PORTAL_OBJECT_PATH)
            .await
            .unwrap();
        tokio::spawn(n.clone().forward_signals(server.clone(), iface));

        let p = proxy(
            &client,
            conchy_portal_proto::PORTAL_OBJECT_PATH,
            "org.freedesktop.impl.portal.Notification",
        )
        .await;
        let body = opts(vec![
            ("title", Value::from("T1")),
            ("default-action", Value::from("open")),
        ]);
        p.call_method("AddNotification", &("org.app", "n1", body))
            .await
            .unwrap();
        // Тот же id → обновление (replaces_id = id у сервера).
        let body = opts(vec![("title", Value::from("T2"))]);
        p.call_method("AddNotification", &("org.app", "n1", body))
            .await
            .unwrap();
        {
            let c = calls.lock().unwrap();
            assert!(
                c[0].starts_with("notify org.app r=0") && c[0].contains("T1"),
                "{c:?}"
            );
            assert!(
                c[1].starts_with("notify org.app r=101"),
                "обновление: {c:?}"
            );
        }

        // Нажатие «default» у сервера → ActionInvoked портала с действием из уведомления.
        let mut sig = p.receive_signal("ActionInvoked").await.unwrap();
        let ifc = client
            .object_server()
            .interface::<_, MockServer>("/org/freedesktop/Notifications")
            .await
            .unwrap();
        // Для T2 действий нет — сначала новое уведомление с действием:
        let body = opts(vec![
            ("title", Value::from("T3")),
            ("default-action", Value::from("open-it")),
        ]);
        p.call_method("AddNotification", &("org.app", "n2", body))
            .await
            .unwrap();
        MockServer::action_invoked(ifc.signal_emitter(), 102, "default")
            .await
            .unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(2), sig.next())
            .await
            .unwrap()
            .unwrap();
        let (app, id, action, _param): (String, String, String, Vec<OwnedValue>) =
            msg.body().deserialize().unwrap();
        assert_eq!(
            (app.as_str(), id.as_str(), action.as_str()),
            ("org.app", "n2", "default")
        );

        p.call_method("RemoveNotification", &("org.app", "n1"))
            .await
            .unwrap();
        assert!(calls.lock().unwrap().last().unwrap() == "close 101");
        // Повторное снятие того же — тихо ничего не делает.
        p.call_method("RemoveNotification", &("org.app", "n1"))
            .await
            .unwrap();
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with("close"))
                .count(),
            1
        );
    }
}
