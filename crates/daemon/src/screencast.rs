//! `org.freedesktop.impl.portal.ScreenCast` (версия 5).
//!
//! * `CreateSession` создаёт объект `Session` и запоминает состояние;
//! * `SelectSources` только запоминает параметры (UI здесь не показываем — так делает и gnome);
//! * `Start` — выбор источников (restore-токен или пикер шелла), затем через
//!   `org.gnome.Mutter.ScreenCast` в conchy-wm открываются потоки PipeWire.
//!
//! Значения курсора у портала и у Mutter разные: 1/2/4 (скрыт/встроен/метаданные) против 0/1/2.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use conchy_portal_proto::cast::{CastPickRequest, CastSource, SOURCE_MONITOR, SOURCE_WINDOW};
use conchy_portal_proto::{SHELL_HOST_BUS_NAME, response};
use futures_util::StreamExt;
use tokio::sync::Notify;
use tracing::{debug, info, warn};
use zbus::{
    Connection, ObjectServer, interface,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

use crate::hosts::{ShellHostProxy, ensure_host};
use crate::mutter::*;
use crate::opts::{Options, get_bool, get_u32};
use crate::request::RequestGuard;
use crate::session::SessionGuard;

pub const VERSION: u32 = 5;
/// Что умеем: мониторы и окна (виртуальные мониторы — позже).
pub const AVAILABLE_SOURCE_TYPES: u32 = SOURCE_MONITOR | SOURCE_WINDOW;
/// Скрыт | встроен | метаданные.
pub const AVAILABLE_CURSOR_MODES: u32 = 1 | 2 | 4;
/// Сколько ждём `PipeWireStreamAdded` от wm.
const STREAM_TIMEOUT: Duration = Duration::from_secs(3);
const RESTORE_VENDOR: &str = "conchy";
const RESTORE_VERSION: u32 = 1;

type Results = HashMap<String, Value<'static>>;

/// Курсор портала → курсор Mutter.
pub fn mutter_cursor_mode(portal: u32) -> u32 {
    match portal {
        2 => 1,
        4 => 2,
        _ => 0,
    }
}

#[derive(Debug, Clone, Default)]
struct Selection {
    types: u32,
    multiple: bool,
    cursor_mode: u32,
    persist_mode: u32,
    restore: Vec<RestoreSource>,
}

/// Источник из restore-токена: монитор по имени выхода, окно по `app_id` (+ заголовку).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreSource {
    pub kind: String,
    pub connector: String,
    pub app_id: String,
    pub title: String,
}

impl RestoreSource {
    fn from_cast(s: &CastSource) -> Self {
        if s.kind == SOURCE_MONITOR {
            Self { kind: "monitor".into(), connector: s.connector.clone(), app_id: String::new(), title: String::new() }
        } else {
            Self { kind: "window".into(), connector: String::new(), app_id: s.app_id.clone(), title: s.title.clone() }
        }
    }
}

/// `restore_data` `(suv)` → источники. Чужой или битый формат — пустой список (покажем пикер).
pub fn parse_restore(v: &Value<'_>) -> Vec<RestoreSource> {
    let Value::Structure(s) = v else { return Vec::new() };
    let f = s.fields();
    let [Value::Str(vendor), Value::U32(version), inner] = f else { return Vec::new() };
    if vendor.as_str() != RESTORE_VENDOR || *version != RESTORE_VERSION {
        return Vec::new();
    }
    let Value::Value(inner) = inner else { return Vec::new() };
    let Value::Array(arr) = &**inner else { return Vec::new() };
    arr.iter()
        .filter_map(|item| {
            let Value::Structure(s) = item else { return None };
            let [Value::Str(kind), Value::Str(connector), Value::Str(app_id), Value::Str(title)] = s.fields() else {
                return None;
            };
            Some(RestoreSource {
                kind: kind.to_string(),
                connector: connector.to_string(),
                app_id: app_id.to_string(),
                title: title.to_string(),
            })
        })
        .collect()
}

pub fn build_restore(sources: &[RestoreSource]) -> Value<'static> {
    let items: Vec<(String, String, String, String)> = sources
        .iter()
        .map(|s| (s.kind.clone(), s.connector.clone(), s.app_id.clone(), s.title.clone()))
        .collect();
    Value::from((RESTORE_VENDOR.to_string(), RESTORE_VERSION, Value::new(items)))
}

/// Окно для restore: единственное с этим `app_id` (при нескольких — единственное с тем же заголовком).
pub fn resolve_window(windows: &HashMap<u64, (String, String)>, app_id: &str, title: &str) -> Option<u64> {
    let same_app: Vec<_> = windows.iter().filter(|(_, (a, _))| a == app_id).collect();
    match same_app.as_slice() {
        [] => None,
        [(id, _)] => Some(**id),
        many => {
            let by_title: Vec<_> = many.iter().filter(|(_, (_, t))| t == title).collect();
            match by_title.as_slice() {
                [(id, _)] => Some(**id),
                _ => None,
            }
        }
    }
}

struct StreamInfo {
    node_id: u32,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    source_type: u32,
}

/// Живая сессия: объект `Session` портала и сессия в wm.
struct CastSession {
    selection: Selection,
    mutter: Option<MutterSessionProxy<'static>>,
    /// wm закрыл сессию сам (сигнал `Closed`) — надо отправить `Closed` порталу.
    backend_closed: Arc<Notify>,
    started: bool,
}

pub struct ScreenCast {
    sessions: Arc<Mutex<HashMap<OwnedObjectPath, CastSession>>>,
    /// В тестах активации нет (p2p-соединение без шины).
    pub activate: bool,
}

impl ScreenCast {
    pub fn new(activate: bool) -> Self {
        Self { sessions: Arc::default(), activate }
    }

    fn with<R>(&self, path: &OwnedObjectPath, f: impl FnOnce(&mut CastSession) -> R) -> Option<R> {
        self.sessions.lock().ok()?.get_mut(path).map(f)
    }
}

fn fail(why: impl std::fmt::Display) -> (u32, Results) {
    warn!("ScreenCast: {why}");
    (response::OTHER, Results::new())
}

/// Открывает потоки в wm. При любой ошибке сессия wm останавливается.
async fn open_streams(
    conn: &Connection,
    sources: &[CastSource],
    cursor_mode: u32,
) -> Result<(MutterSessionProxy<'static>, Vec<StreamInfo>), String> {
    let sc = MutterScreenCastProxy::new(conn).await.map_err(|e| format!("нет Mutter.ScreenCast: {e}"))?;
    let path = sc.create_session(HashMap::new()).await.map_err(|e| format!("CreateSession: {e}"))?;
    let session = MutterSessionProxy::builder(conn)
        .path(path)
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;
    let result = record_all(conn, &session, sources, cursor_mode).await;
    match result {
        Ok(streams) => Ok((session, streams)),
        Err(e) => {
            let _ = session.stop().await;
            Err(e)
        }
    }
}

async fn record_all(
    conn: &Connection,
    session: &MutterSessionProxy<'static>,
    sources: &[CastSource],
    cursor_mode: u32,
) -> Result<Vec<StreamInfo>, String> {
    let mut streams = Vec::new();
    for src in sources {
        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
        props.insert("cursor-mode", Value::from(mutter_cursor_mode(cursor_mode)));
        let path = if src.kind == SOURCE_WINDOW {
            props.insert("window-id", Value::from(src.window_id));
            session.record_window(props).await
        } else {
            session.record_monitor(&src.connector, props).await
        }
        .map_err(|e| format!("запись источника: {e}"))?;
        let stream = MutterStreamProxy::builder(conn)
            .path(path)
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string())?;
        // Подписка ДО Start: иначе сигнал можно пропустить.
        let added = stream.receive_pipe_wire_stream_added().await.map_err(|e| e.to_string())?;
        streams.push((src.kind, stream, added));
    }
    session.start().await.map_err(|e| format!("Session.Start: {e}"))?;
    let mut out = Vec::new();
    for (kind, stream, mut added) in streams {
        let signal = tokio::time::timeout(STREAM_TIMEOUT, added.next())
            .await
            .map_err(|_| "wm не создал поток PipeWire за 3 с".to_string())?
            .ok_or("поток сигналов закрыт")?;
        let node_id = signal.args().map_err(|e| e.to_string())?.node_id;
        let params = stream.parameters().await.unwrap_or_default();
        let pair = |k: &str| -> Option<(i32, i32)> {
            let v = params.get(k)?;
            let Value::Structure(s) = &**v else { return None };
            match s.fields() {
                [Value::I32(a), Value::I32(b)] => Some((*a, *b)),
                _ => None,
            }
        };
        out.push(StreamInfo { node_id, position: pair("position"), size: pair("size"), source_type: kind });
    }
    Ok(out)
}

impl ScreenCast {
    /// Источники из restore-токена, если все ещё существуют.
    async fn restore_sources(&self, conn: &Connection, restore: &[RestoreSource]) -> Option<Vec<CastSource>> {
        if restore.is_empty() {
            return None;
        }
        let mut windows: Option<HashMap<u64, (String, String)>> = None;
        let mut out = Vec::new();
        for r in restore {
            if r.kind == "monitor" {
                out.push(CastSource { kind: SOURCE_MONITOR, connector: r.connector.clone(), ..Default::default() });
            } else {
                if windows.is_none() {
                    let proxy = ShellIntrospectProxy::new(conn).await.ok()?;
                    let raw = proxy.get_windows().await.ok()?;
                    let text = |m: &HashMap<String, OwnedValue>, k: &str| match m.get(k).map(|v| &**v) {
                        Some(Value::Str(s)) => s.to_string(),
                        _ => String::new(),
                    };
                    windows = Some(raw.iter().map(|(id, m)| (*id, (text(m, "app-id"), text(m, "title")))).collect());
                }
                let id = resolve_window(windows.as_ref()?, &r.app_id, &r.title)?;
                out.push(CastSource {
                    kind: SOURCE_WINDOW,
                    window_id: id,
                    app_id: r.app_id.clone(),
                    title: r.title.clone(),
                    ..Default::default()
                });
            }
        }
        Some(out)
    }
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: String,
        _options: Options,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let handle = OwnedObjectPath::from(handle);
        let session_handle = OwnedObjectPath::from(session_handle);
        let (_guard, _cancel) = match RequestGuard::new(server, handle).await {
            Ok(g) => g,
            Err(e) => return fail(e),
        };
        let guard = match SessionGuard::new(server, session_handle.clone(), 1).await {
            Ok(g) => g,
            Err(e) => return fail(e),
        };
        let backend_closed = Arc::new(Notify::new());
        if let Ok(mut map) = self.sessions.lock() {
            map.insert(
                session_handle.clone(),
                CastSession { selection: Selection::default(), mutter: None, backend_closed: backend_closed.clone(), started: false },
            );
        }
        info!(session = %session_handle, app_id, "ScreenCast.CreateSession");

        // Жизнь сессии: ждём `Close` от фронтенда или закрытие в wm; потом гасим поток в wm.
        let sessions = self.sessions.clone();
        let path = session_handle.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = guard.wait_closed() => {}
                _ = backend_closed.notified() => {
                    let _ = guard.close_by_backend().await;
                }
            }
            let mutter = sessions.lock().ok().and_then(|mut m| m.remove(&path)).and_then(|s| s.mutter);
            if let Some(m) = mutter {
                let _ = m.stop().await;
            }
            debug!(session = %path, "сессия ScreenCast закрыта");
        });

        let mut r = Results::new();
        r.insert("session_id".into(), Value::from(session_handle.to_string()));
        (response::OK, r)
    }

    async fn select_sources(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        _app_id: String,
        options: Options,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let (_guard, _cancel) = match RequestGuard::new(server, OwnedObjectPath::from(handle)).await {
            Ok(g) => g,
            Err(e) => return fail(e),
        };
        let path = OwnedObjectPath::from(session_handle);
        let restore = crate::opts::get_value(&options, "restore_data").map(parse_restore).unwrap_or_default();
        let sel = Selection {
            types: get_u32(&options, "types").filter(|t| *t != 0).unwrap_or(SOURCE_MONITOR) & AVAILABLE_SOURCE_TYPES,
            multiple: get_bool(&options, "multiple").unwrap_or(false),
            cursor_mode: get_u32(&options, "cursor_mode").unwrap_or(1),
            persist_mode: get_u32(&options, "persist_mode").unwrap_or(0).min(2),
            restore,
        };
        match self.with(&path, |s| s.selection = sel) {
            Some(()) => (response::OK, Results::new()),
            None => fail(format!("нет сессии {path}")),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        _options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let started = Instant::now();
        let handle = OwnedObjectPath::from(handle);
        let path = OwnedObjectPath::from(session_handle);
        let (_guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => return fail(e),
        };
        let Some((selection, already)) = self.with(&path, |s| (s.selection.clone(), s.started)) else {
            return fail(format!("нет сессии {path}"));
        };
        if already {
            return fail("сессия уже запущена");
        }
        info!(%handle, session = %path, app_id, types = selection.types, multiple = selection.multiple, "ScreenCast.Start");

        let id = handle.to_string();
        let work = async {
            // 1. Источники: restore-токен или пикер шелла.
            let sources = match self.restore_sources(conn, &selection.restore).await {
                Some(s) => {
                    info!("источники восстановлены по restore-токену, пикер пропущен");
                    s
                }
                None => {
                    if self.activate
                        && let Err(e) = ensure_host(conn, SHELL_HOST_BUS_NAME).await {
                            return fail(format!("нет хоста conchy-shell: {e}"));
                        }
                    let host = match ShellHostProxy::new(conn).await {
                        Ok(h) => h,
                        Err(e) => return fail(format!("прокси к хосту: {e}")),
                    };
                    let req = CastPickRequest {
                        app_id: app_id.clone(),
                        parent_window: parent_window.clone(),
                        types: selection.types,
                        multiple: selection.multiple,
                    };
                    match host.pick_cast_sources(&id, &req).await {
                        Ok((code, res)) if code == response::OK && !res.sources.is_empty() => {
                            if selection.multiple { res.sources } else { res.sources.into_iter().take(1).collect() }
                        }
                        Ok((code, _)) if code != response::OK => return (code, Results::new()),
                        Ok(_) => return (response::CANCELLED, Results::new()),
                        Err(e) => return fail(format!("хост ответил ошибкой: {e}")),
                    }
                }
            };

            // 2. Потоки в wm.
            let (mutter, streams) = match open_streams(conn, &sources, selection.cursor_mode).await {
                Ok(v) => v,
                Err(e) => return fail(e),
            };

            // 3. Сессия wm закрылась сама — сообщим порталу.
            let backend_closed = self.with(&path, |s| s.backend_closed.clone());
            if let (Ok(mut closed), Some(notify)) = (mutter.receive_closed().await, backend_closed) {
                tokio::spawn(async move {
                    if closed.next().await.is_some() {
                        notify.notify_one();
                    }
                });
            }
            if self.with(&path, |s| {
                s.mutter = Some(mutter.clone());
                s.started = true;
            })
            .is_none()
            {
                // Сессию успели закрыть, пока открывались потоки.
                let _ = mutter.stop().await;
                return (response::CANCELLED, Results::new());
            }

            // 4. Ответ.
            let stream_values: Vec<(u32, HashMap<String, Value<'static>>)> = streams
                .iter()
                .zip(&sources)
                .map(|(s, src)| {
                    let mut d: HashMap<String, Value<'static>> = HashMap::new();
                    if let Some(p) = s.position {
                        d.insert("position".into(), Value::from(p));
                    }
                    if let Some(sz) = s.size {
                        d.insert("size".into(), Value::from(sz));
                    }
                    d.insert("source_type".into(), Value::from(s.source_type));
                    let sid = if src.kind == SOURCE_WINDOW { format!("window:{}", src.window_id) } else { format!("monitor:{}", src.connector) };
                    d.insert("id".into(), Value::from(sid));
                    (s.node_id, d)
                })
                .collect();
            let mut r = Results::new();
            r.insert("streams".into(), Value::from(stream_values));
            if selection.persist_mode > 0 {
                let restore: Vec<RestoreSource> = sources.iter().map(RestoreSource::from_cast).collect();
                r.insert("persist_mode".into(), Value::from(selection.persist_mode));
                r.insert("restore_data".into(), build_restore(&restore));
            }
            (response::OK, r)
        };
        let out = tokio::select! {
            r = work => r,
            _ = cancel => {
                if let Ok(host) = ShellHostProxy::new(conn).await {
                    let _ = host.cancel(&id).await;
                }
                (response::CANCELLED, Results::new())
            }
        };
        info!(%handle, response = out.0, ms = started.elapsed().as_secs_f64() * 1000.0, "ScreenCast.Start готов");
        out
    }

    #[zbus(property, name = "AvailableSourceTypes")]
    fn available_source_types(&self) -> u32 {
        AVAILABLE_SOURCE_TYPES
    }

    #[zbus(property, name = "AvailableCursorModes")]
    fn available_cursor_modes(&self) -> u32 {
        AVAILABLE_CURSOR_MODES
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

#[cfg(test)]
mod tests;
