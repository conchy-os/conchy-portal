use super::*;
use crate::testutil::{p2p_pair, proxy};
use conchy_portal_proto::cast::{CastPickRequest, CastPickResult};
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::Notify;
use zbus::{fdo, object_server::SignalEmitter};

const IFACE: &str = "org.freedesktop.impl.portal.ScreenCast";

type Log = Arc<Mutex<Vec<String>>>;

// ---- мок conchy-wm: Mutter.ScreenCast ----

#[derive(Clone)]
struct MockMutter {
    log: Log,
    next: Arc<AtomicU32>,
    fail_record: bool,
    silent: bool,
}

struct MockSession {
    log: Log,
    next: Arc<AtomicU32>,
    streams: Arc<Mutex<Vec<(OwnedObjectPath, u32)>>>,
    fail_record: bool,
    silent: bool,
}

struct MockStream {
    size: (i32, i32),
}

#[interface(name = "org.gnome.Mutter.ScreenCast")]
impl MockMutter {
    async fn create_session(
        &self,
        _props: HashMap<&str, Value<'_>>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        self.log.lock().unwrap().push("create".into());
        let path = OwnedObjectPath::try_from(format!("/org/gnome/Mutter/ScreenCast/Session/u{}", self.next.fetch_add(1, Ordering::SeqCst))).unwrap();
        let s = MockSession {
            log: self.log.clone(),
            next: self.next.clone(),
            streams: Arc::default(),
            fail_record: self.fail_record,
            silent: self.silent,
        };
        server.at(&path, s).await.map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok(path)
    }
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Session")]
impl MockSession {
    async fn start(&self, #[zbus(object_server)] server: &ObjectServer) {
        self.log.lock().unwrap().push("start".into());
        if self.silent {
            return;
        }
        let streams = self.streams.lock().unwrap().clone();
        for (path, node) in streams {
            if let Ok(iface) = server.interface::<_, MockStream>(&path).await {
                let _ = MockStream::pipe_wire_stream_added(iface.signal_emitter(), node).await;
            }
        }
    }
    async fn stop(&self, #[zbus(signal_context)] ctxt: SignalEmitter<'_>) {
        self.log.lock().unwrap().push("stop".into());
        let _ = MockSession::closed(&ctxt).await;
    }
    async fn record_monitor(
        &self,
        connector: &str,
        props: HashMap<&str, Value<'_>>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        if self.fail_record {
            return Err(fdo::Error::Failed("no such monitor".into()));
        }
        let cursor = props.get("cursor-mode").and_then(|v| u32::try_from(v).ok()).unwrap_or(99);
        self.log.lock().unwrap().push(format!("record_monitor {connector} cursor={cursor}"));
        self.add_stream(server, (1920, 1080)).await
    }
    async fn record_window(
        &self,
        props: HashMap<&str, Value<'_>>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        let cursor = props.get("cursor-mode").and_then(|v| u32::try_from(v).ok()).unwrap_or(99);
        let id = props.get("window-id").and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
        self.log.lock().unwrap().push(format!("record_window {id} cursor={cursor}"));
        self.add_stream(server, (1, 1)).await
    }
    #[zbus(signal)]
    async fn closed(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

impl MockSession {
    async fn add_stream(&self, server: &ObjectServer, size: (i32, i32)) -> fdo::Result<OwnedObjectPath> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let path = OwnedObjectPath::try_from(format!("/org/gnome/Mutter/ScreenCast/Stream/u{n}")).unwrap();
        server.at(&path, MockStream { size }).await.map_err(|e| fdo::Error::Failed(e.to_string()))?;
        self.streams.lock().unwrap().push((path.clone(), 1000 + n));
        Ok(path)
    }
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Stream")]
impl MockStream {
    #[zbus(signal)]
    async fn pipe_wire_stream_added(ctxt: &SignalEmitter<'_>, node_id: u32) -> zbus::Result<()>;

    #[zbus(property)]
    fn parameters(&self) -> HashMap<String, Value<'static>> {
        HashMap::from([
            ("position".to_string(), Value::from((0i32, 0i32))),
            ("size".to_string(), Value::from(self.size)),
        ])
    }
}

// ---- мок Introspect и ShellHost ----

struct MockIntrospect(HashMap<u64, (String, String)>);

#[interface(name = "org.gnome.Shell.Introspect")]
impl MockIntrospect {
    async fn get_windows(&self) -> HashMap<u64, HashMap<String, Value<'static>>> {
        self.0
            .iter()
            .map(|(id, (app, title))| {
                (*id, HashMap::from([("app-id".to_string(), Value::from(app.clone())), ("title".to_string(), Value::from(title.clone()))]))
            })
            .collect()
    }
}

#[derive(Default)]
struct MockShell {
    picks: Arc<Mutex<Vec<CastPickRequest>>>,
    answer: Option<(u32, Vec<CastSource>)>,
    block: bool,
    stop: Arc<Notify>,
    cancelled: Arc<Mutex<Vec<String>>>,
}

#[interface(name = "org.conchy.Portal.ShellHost")]
impl MockShell {
    async fn pick_cast_sources(&self, _id: &str, req: CastPickRequest) -> (u32, CastPickResult) {
        self.picks.lock().unwrap().push(req);
        if self.block {
            self.stop.notified().await;
            return (1, CastPickResult::default());
        }
        let (code, sources) = self.answer.clone().unwrap_or((0, vec![]));
        (code, CastPickResult { sources })
    }
    async fn cancel(&self, id: &str) {
        self.cancelled.lock().unwrap().push(id.to_owned());
        self.stop.notify_one();
    }
}

fn monitor(c: &str) -> CastSource {
    CastSource { kind: SOURCE_MONITOR, connector: c.into(), ..Default::default() }
}

fn window(id: u64, app: &str, title: &str) -> CastSource {
    CastSource { kind: SOURCE_WINDOW, window_id: id, app_id: app.into(), title: title.into(), ..Default::default() }
}

struct Env {
    server: Connection,
    client: Connection,
    log: Log,
    picks: Arc<Mutex<Vec<CastPickRequest>>>,
    cancelled: Arc<Mutex<Vec<String>>>,
}

async fn env(shell: MockShell, mutter_fail: bool, silent: bool, windows: Vec<(u64, &str, &str)>) -> Env {
    let (server, client) = p2p_pair().await;
    server.object_server().at(conchy_portal_proto::PORTAL_OBJECT_PATH, ScreenCast::new(false)).await.unwrap();
    let log: Log = Log::default();
    let m = MockMutter { log: log.clone(), next: Arc::new(AtomicU32::new(1)), fail_record: mutter_fail, silent };
    client.object_server().at("/org/gnome/Mutter/ScreenCast", m).await.unwrap();
    let w = windows.into_iter().map(|(i, a, t)| (i, (a.to_string(), t.to_string()))).collect();
    client.object_server().at("/org/gnome/Shell/Introspect", MockIntrospect(w)).await.unwrap();
    let picks = shell.picks.clone();
    let cancelled = shell.cancelled.clone();
    client.object_server().at("/org/conchy/Portal", shell).await.unwrap();
    Env { server, client, log, picks, cancelled }
}

type Reply = (u32, HashMap<String, OwnedValue>);

async fn call(e: &Env, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Reply {
    let p = proxy(&e.client, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
    p.call(method, body).await.unwrap()
}

async fn create_and_select(e: &Env, session: &str, opts: Options) {
    let h = ObjectPath::try_from("/r/c").unwrap();
    let s = ObjectPath::try_from(session).unwrap();
    let (code, res) = call(e, "CreateSession", &(h, s.clone(), "org.test", Options::new())).await;
    assert_eq!(code, 0);
    assert_eq!(String::try_from(res["session_id"].clone()).unwrap(), session);
    let h = ObjectPath::try_from("/r/s").unwrap();
    let (code, _) = call(e, "SelectSources", &(h, s, "org.test", opts)).await;
    assert_eq!(code, 0);
}

fn opts(items: Vec<(&str, Value<'static>)>) -> Options {
    items.into_iter().map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).unwrap())).collect()
}

async fn start(e: &Env, session: &str, handle: &str) -> Reply {
    let h = ObjectPath::try_from(handle).unwrap();
    let s = ObjectPath::try_from(session).unwrap();
    call(e, "Start", &(h, s, "org.test", "", Options::new())).await
}

fn streams_of(r: &Reply) -> Vec<(u32, HashMap<String, OwnedValue>)> {
    r.1["streams"].clone().try_into().unwrap()
}

#[test]
fn cursor_mode_mapping() {
    assert_eq!([mutter_cursor_mode(1), mutter_cursor_mode(2), mutter_cursor_mode(4), mutter_cursor_mode(0)], [0, 1, 2, 0]);
}

#[test]
fn restore_roundtrip_and_garbage() {
    let src = vec![
        RestoreSource { kind: "monitor".into(), connector: "eDP-1".into(), app_id: String::new(), title: String::new() },
        RestoreSource { kind: "window".into(), connector: String::new(), app_id: "firefox.desktop".into(), title: "Tab".into() },
    ];
    let v = build_restore(&src);
    assert_eq!(parse_restore(&v), src);
    assert!(parse_restore(&Value::from("x")).is_empty());
    assert!(parse_restore(&Value::from(("other".to_string(), 1u32, Value::new(0u32)))).is_empty());
    assert!(parse_restore(&Value::from((RESTORE_VENDOR.to_string(), 99u32, Value::new(0u32)))).is_empty());
}

#[test]
fn window_resolution() {
    let w: HashMap<u64, (String, String)> = HashMap::from([
        (1, ("a.desktop".into(), "One".into())),
        (2, ("b.desktop".into(), "X".into())),
        (3, ("b.desktop".into(), "Y".into())),
        (4, ("c.desktop".into(), "Same".into())),
        (5, ("c.desktop".into(), "Same".into())),
    ]);
    assert_eq!(resolve_window(&w, "a.desktop", "other"), Some(1));
    assert_eq!(resolve_window(&w, "b.desktop", "Y"), Some(3));
    assert_eq!(resolve_window(&w, "b.desktop", "Z"), None, "неоднозначно");
    assert_eq!(resolve_window(&w, "c.desktop", "Same"), None, "два одинаковых");
    assert_eq!(resolve_window(&w, "zzz", ""), None);
}

#[tokio::test]
async fn start_with_picker_opens_streams() {
    let shell = MockShell { answer: Some((0, vec![monitor("eDP-1"), window(77, "x.desktop", "T")])), ..Default::default() };
    let e = env(shell, false, false, vec![]).await;
    create_and_select(
        &e,
        "/s/1",
        opts(vec![("types", Value::from(3u32)), ("multiple", Value::from(true)), ("cursor_mode", Value::from(2u32)), ("persist_mode", Value::from(2u32))]),
    )
    .await;
    let r = start(&e, "/s/1", "/r/start").await;
    assert_eq!(r.0, 0);
    let streams = streams_of(&r);
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0].0, 1000 + 2, "node id из сигнала wm");
    let size: (i32, i32) = streams[0].1["size"].clone().try_into().unwrap();
    assert_eq!(size, (1920, 1080));
    assert_eq!(u32::try_from(streams[0].1["source_type"].clone()).unwrap(), 1);
    assert_eq!(u32::try_from(streams[1].1["source_type"].clone()).unwrap(), 2);
    let log = e.log.lock().unwrap().clone();
    assert_eq!(log, ["create", "record_monitor eDP-1 cursor=1", "record_window 77 cursor=1", "start"].map(String::from));
    assert_eq!(u32::try_from(r.1["persist_mode"].clone()).unwrap(), 2);
    let restore = parse_restore(&Value::try_from(&r.1["restore_data"]).unwrap());
    assert_eq!(restore.len(), 2);
    let req = e.picks.lock().unwrap()[0].clone();
    assert_eq!((req.types, req.multiple), (3, true));
}

#[tokio::test]
async fn single_selection_takes_first_and_no_persist_means_no_token() {
    let shell = MockShell { answer: Some((0, vec![monitor("A"), monitor("B")])), ..Default::default() };
    let e = env(shell, false, false, vec![]).await;
    create_and_select(&e, "/s/2", opts(vec![])).await;
    let r = start(&e, "/s/2", "/r/start").await;
    assert_eq!(streams_of(&r).len(), 1);
    assert!(!r.1.contains_key("restore_data"));
    assert_eq!(e.log.lock().unwrap()[1], "record_monitor A cursor=0");
}

#[tokio::test]
async fn restore_token_skips_picker_and_resolves_window() {
    let e = env(MockShell::default(), false, false, vec![(42, "firefox.desktop", "Meet"), (43, "other.desktop", "x")]).await;
    let token = build_restore(&[
        RestoreSource { kind: "window".into(), connector: String::new(), app_id: "firefox.desktop".into(), title: "Meet".into() },
        RestoreSource { kind: "monitor".into(), connector: "DP-2".into(), app_id: String::new(), title: String::new() },
    ]);
    create_and_select(&e, "/s/3", opts(vec![("restore_data", token), ("persist_mode", Value::from(2u32)), ("multiple", Value::from(true))])).await;
    let r = start(&e, "/s/3", "/r/start").await;
    assert_eq!(r.0, 0);
    assert!(e.picks.lock().unwrap().is_empty(), "пикер не должен показываться");
    let log = e.log.lock().unwrap().clone();
    assert!(log.contains(&"record_window 42 cursor=0".to_string()) && log.contains(&"record_monitor DP-2 cursor=0".to_string()), "{log:?}");
}

#[tokio::test]
async fn stale_restore_falls_back_to_picker() {
    let shell = MockShell { answer: Some((0, vec![monitor("eDP-1")])), ..Default::default() };
    let e = env(shell, false, false, vec![(1, "other.desktop", "x")]).await;
    let token = build_restore(&[RestoreSource { kind: "window".into(), connector: String::new(), app_id: "gone.desktop".into(), title: "t".into() }]);
    create_and_select(&e, "/s/4", opts(vec![("restore_data", token)])).await;
    assert_eq!(start(&e, "/s/4", "/r/start").await.0, 0);
    assert_eq!(e.picks.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn picker_cancel_and_host_error() {
    let shell = MockShell { answer: Some((1, vec![])), ..Default::default() };
    let e = env(shell, false, false, vec![]).await;
    create_and_select(&e, "/s/5", opts(vec![])).await;
    assert_eq!(start(&e, "/s/5", "/r/start").await.0, 1);
    assert!(e.log.lock().unwrap().is_empty(), "wm не трогали");
}

#[tokio::test]
async fn mutter_failure_is_response_2_and_session_stopped() {
    let shell = MockShell { answer: Some((0, vec![monitor("ghost")])), ..Default::default() };
    let e = env(shell, true, false, vec![]).await;
    create_and_select(&e, "/s/6", opts(vec![])).await;
    assert_eq!(start(&e, "/s/6", "/r/start").await.0, 2);
    assert_eq!(e.log.lock().unwrap().last().unwrap(), "stop");
}

#[tokio::test]
async fn silent_wm_times_out_with_2() {
    let shell = MockShell { answer: Some((0, vec![monitor("eDP-1")])), ..Default::default() };
    let e = env(shell, false, true, vec![]).await;
    create_and_select(&e, "/s/7", opts(vec![])).await;
    let t = Instant::now();
    let r = start(&e, "/s/7", "/r/start").await;
    assert_eq!(r.0, 2);
    assert!(t.elapsed() >= STREAM_TIMEOUT, "ждём поток ровно таймаут");
    assert_eq!(e.log.lock().unwrap().last().unwrap(), "stop");
}

#[tokio::test]
async fn session_close_stops_wm_stream_and_wm_close_emits_closed() {
    let shell = MockShell { answer: Some((0, vec![monitor("eDP-1")])), ..Default::default() };
    let e = env(shell, false, false, vec![]).await;
    create_and_select(&e, "/s/8", opts(vec![])).await;
    assert_eq!(start(&e, "/s/8", "/r/start").await.0, 0);
    let sess = proxy(&e.client, "/s/8", "org.freedesktop.impl.portal.Session").await;
    sess.call_method("Close", &()).await.unwrap();
    for _ in 0..100 {
        if e.log.lock().unwrap().last().map(String::as_str) == Some("stop") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(e.log.lock().unwrap().last().unwrap(), "stop", "Session.Close должен гасить поток в wm");

    // Второй сеанс: wm закрывает сессию сам → портал получает Closed.
    let shell = MockShell { answer: Some((0, vec![monitor("eDP-1")])), ..Default::default() };
    let e2 = env(shell, false, false, vec![]).await;
    create_and_select(&e2, "/s/9", opts(vec![])).await;
    assert_eq!(start(&e2, "/s/9", "/r/start").await.0, 0);
    let sess2 = proxy(&e2.client, "/s/9", "org.freedesktop.impl.portal.Session").await;
    let mut closed = sess2.receive_signal("Closed").await.unwrap();
    let mpath = "/org/gnome/Mutter/ScreenCast/Session/u1";
    let iface = e2.client.object_server().interface::<_, MockSession>(mpath).await.unwrap();
    MockSession::closed(iface.signal_emitter()).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), closed.next()).await;
    assert!(matches!(got, Ok(Some(_))), "Closed от wm должен дойти до портала");
}

#[tokio::test]
async fn request_close_cancels_picker() {
    let shell = MockShell { block: true, ..Default::default() };
    let e = env(shell, false, false, vec![]).await;
    create_and_select(&e, "/s/10", opts(vec![])).await;
    let (cancelled, picks) = (e.cancelled.clone(), e.picks.clone());
    let client = e.client.clone();
    let call = tokio::spawn(async move {
        let p = proxy(&client, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/startx").unwrap();
        let s = ObjectPath::try_from("/s/10").unwrap();
        p.call::<_, _, Reply>("Start", &(h, s, "org.test", "", Options::new())).await.unwrap()
    });
    for _ in 0..100 {
        if !picks.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let req = proxy(&e.client, "/r/startx", "org.freedesktop.impl.portal.Request").await;
    req.call_method("Close", &()).await.unwrap();
    let (code, _) = tokio::time::timeout(Duration::from_secs(2), call).await.unwrap().unwrap();
    assert_eq!(code, 1);
    assert_eq!(cancelled.lock().unwrap().as_slice(), ["/r/startx"]);
    drop(e.server);
}

#[tokio::test]
async fn properties() {
    let e = env(MockShell::default(), false, false, vec![]).await;
    let p = proxy(&e.client, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
    assert_eq!(p.get_property::<u32>("AvailableSourceTypes").await.unwrap(), 3);
    assert_eq!(p.get_property::<u32>("AvailableCursorModes").await.unwrap(), 7);
    assert_eq!(p.get_property::<u32>("version").await.unwrap(), 5);
}
