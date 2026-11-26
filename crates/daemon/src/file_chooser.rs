//! `org.freedesktop.impl.portal.FileChooser`: разбирает `a{sv}`, передаёт запрос хосту
//! (`conchy-files`, `org.conchy.Portal.FilesHost`) и собирает ответ порталу.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Instant;

use conchy_portal_proto::{FILES_HOST_BUS_NAME, files::*, response};
use tracing::{info, warn};
use zbus::{
    Connection, ObjectServer, interface,
    zvariant::{ObjectPath, OwnedObjectPath, Value},
};

use crate::hosts::{FilesHostProxy, ensure_host};
use crate::opts::{self, Options, get_bool, get_str};
use crate::request::RequestGuard;
use crate::uri::path_to_uri;

pub const VERSION: u32 = 5;

type Results = HashMap<String, Value<'static>>;

#[derive(Default)]
pub struct FileChooser {
    /// В тестах активации нет (p2p-соединение без шины).
    pub activate: bool,
}

impl FileChooser {
    pub fn new() -> Self {
        Self { activate: true }
    }
}

/// Собирает [`FilePickRequest`] из `a{sv}` портала. Неизвестные ключи игнорируются.
pub fn build_request(
    kind: u32,
    app_id: String,
    parent_window: String,
    title: String,
    o: &Options,
) -> FilePickRequest {
    let value = |k: &str| opts::get_value(o, k);
    FilePickRequest {
        kind,
        app_id,
        parent_window,
        title,
        accept_label: get_str(o, "accept_label").unwrap_or_default(),
        modal: get_bool(o, "modal").unwrap_or(true),
        multiple: get_bool(o, "multiple").unwrap_or(false),
        directory: get_bool(o, "directory").unwrap_or(false),
        filters: value("filters")
            .map(opts::parse_filters)
            .unwrap_or_default(),
        current_filter: value("current_filter")
            .and_then(opts::parse_filter)
            .into_iter()
            .collect(),
        choices: value("choices")
            .map(opts::parse_choices)
            .unwrap_or_default(),
        current_folder: value("current_folder")
            .and_then(opts::parse_bytes)
            .unwrap_or_default(),
        current_name: get_str(o, "current_name").unwrap_or_default(),
        current_file: value("current_file")
            .and_then(opts::parse_bytes)
            .unwrap_or_default(),
        files: value("files")
            .map(opts::parse_byte_arrays)
            .unwrap_or_default(),
    }
}

/// Ответ хоста → `results` портала.
pub fn build_results(r: FilePickResult) -> Results {
    let mut out = Results::new();
    let uris: Vec<String> = r
        .paths
        .iter()
        .map(|p| path_to_uri(Path::new(OsStr::from_bytes(p))))
        .collect();
    out.insert("uris".into(), Value::from(uris));
    let choices: Vec<(String, String)> = r.choices.into_iter().map(|c| (c.id, c.value)).collect();
    out.insert("choices".into(), Value::from(choices));
    if let Some(f) = r.current_filter.into_iter().next() {
        let rules: Vec<(u32, String)> = f.rules.into_iter().map(|r| (r.kind, r.pattern)).collect();
        out.insert("current_filter".into(), Value::from((f.name, rules)));
    }
    out.insert("writable".into(), Value::from(r.writable));
    out
}

impl FileChooser {
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        kind: u32,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
        conn: &Connection,
        server: &ObjectServer,
    ) -> (u32, Results) {
        let started = Instant::now();
        let handle = OwnedObjectPath::from(handle);
        let (_guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => {
                warn!("FileChooser: {e}");
                return (response::OTHER, Results::new());
            }
        };
        let req = build_request(kind, app_id, parent_window, title, &options);
        info!(%handle, kind, app_id = %req.app_id, multiple = req.multiple, directory = req.directory, "FileChooser");

        if self.activate
            && let Err(e) = ensure_host(conn, FILES_HOST_BUS_NAME).await
        {
            warn!("FileChooser: нет хоста conchy-files: {e}");
            return (response::OTHER, Results::new());
        }
        let host = match FilesHostProxy::new(conn).await {
            Ok(h) => h,
            Err(e) => {
                warn!("FileChooser: прокси к хосту: {e}");
                return (response::OTHER, Results::new());
            }
        };
        let id = handle.to_string();
        let out = tokio::select! {
            r = host.pick(&id, &req) => match r {
                Ok((code, result)) if code == response::OK => (code, build_results(result)),
                Ok((code, _)) => (code, Results::new()),
                Err(e) => {
                    warn!("FileChooser: хост ответил ошибкой: {e}");
                    (response::OTHER, Results::new())
                }
            },
            _ = cancel => {
                let _ = host.cancel(&id).await;
                (response::CANCELLED, Results::new())
            }
        };
        info!(%handle, response = out.0, ms = started.elapsed().as_secs_f64() * 1000.0, "FileChooser готов");
        out
    }
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    #[allow(clippy::too_many_arguments)]
    async fn open_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        self.run(
            KIND_OPEN,
            handle,
            app_id,
            parent_window,
            title,
            options,
            conn,
            server,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_file(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        self.run(
            KIND_SAVE,
            handle,
            app_id,
            parent_window,
            title,
            options,
            conn,
            server,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_files(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        self.run(
            KIND_SAVE_FILES,
            handle,
            app_id,
            parent_window,
            title,
            options,
            conn,
            server,
        )
        .await
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
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Notify;
    use zbus::zvariant::OwnedValue;

    const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";

    /// Мок `org.conchy.Portal.FilesHost`.
    #[derive(Default)]
    struct MockHost {
        seen: Arc<Mutex<Vec<(String, FilePickRequest)>>>,
        cancelled: Arc<Mutex<Vec<String>>>,
        block: bool,
        stop: Arc<Notify>,
        result: FilePickResult,
        code: u32,
        fail: bool,
    }

    #[interface(name = "org.conchy.Portal.FilesHost")]
    impl MockHost {
        async fn pick(
            &self,
            id: &str,
            req: FilePickRequest,
        ) -> zbus::fdo::Result<(u32, FilePickResult)> {
            self.seen.lock().unwrap().push((id.to_owned(), req));
            if self.fail {
                return Err(zbus::fdo::Error::Failed("host crashed".into()));
            }
            if self.block {
                self.stop.notified().await;
                return Ok((response::CANCELLED, FilePickResult::default()));
            }
            Ok((self.code, self.result.clone()))
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
            .at(
                conchy_portal_proto::PORTAL_OBJECT_PATH,
                FileChooser::default(),
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

    fn opts_with(entries: Vec<(&str, Value<'static>)>) -> Options {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).unwrap()))
            .collect()
    }

    #[tokio::test]
    async fn open_file_parses_options_and_builds_uris() {
        let host = MockHost {
            result: FilePickResult {
                paths: vec![b"/home/u/a b.png".to_vec(), vec![b'/', 0xff, b'x']],
                choices: vec![ChoiceSelection {
                    id: "enc".into(),
                    value: "utf8".into(),
                }],
                current_filter: vec![Filter {
                    name: "Images".into(),
                    rules: vec![FilterRule {
                        kind: 0,
                        pattern: "*.png".into(),
                    }],
                }],
                writable: true,
            },
            ..Default::default()
        };
        let seen = host.seen.clone();
        let (_s, c) = setup(host).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let options = opts_with(vec![
            ("multiple", Value::from(true)),
            ("accept_label", Value::from("Attach")),
            (
                "filters",
                Value::from(vec![(
                    "Images".to_owned(),
                    vec![(0u32, "*.png".to_owned())],
                )]),
            ),
            ("current_folder", Value::from(b"/home/u\0".to_vec())),
            ("unknown_key", Value::from(1u32)),
            ("directory", Value::from("wrong type")),
        ]);
        let h = ObjectPath::try_from("/r/f1").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) = p
            .call("OpenFile", &(h, "org.app", "wayland:abc", "Pick", options))
            .await
            .unwrap();
        assert_eq!(code, 0);
        let uris: Vec<String> = res["uris"].clone().try_into().unwrap();
        assert_eq!(
            uris,
            vec![
                "file:///home/u/a%20b.png".to_owned(),
                "file:///%FFx".to_owned()
            ]
        );
        assert_eq!(res["writable"].clone().try_into(), Ok(true));

        let (id, req) = seen.lock().unwrap()[0].clone();
        assert_eq!(id, "/r/f1");
        assert_eq!(req.kind, KIND_OPEN);
        assert!(req.multiple && !req.directory);
        assert_eq!(req.accept_label, "Attach");
        assert_eq!(req.parent_window, "wayland:abc");
        assert_eq!(req.filters[0].rules[0].pattern, "*.png");
        assert_eq!(req.current_folder, b"/home/u");
    }

    #[tokio::test]
    async fn save_kinds_and_fields() {
        let host = MockHost::default();
        let seen = host.seen.clone();
        let (_s, c) = setup(host).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let options = opts_with(vec![
            ("current_name", Value::from("report.txt")),
            ("current_file", Value::from(b"/tmp/old.txt\0".to_vec())),
            ("files", Value::from(vec![b"a.txt\0".to_vec()])),
        ]);
        let h = ObjectPath::try_from("/r/f2").unwrap();
        let _: (u32, HashMap<String, OwnedValue>) = p
            .call("SaveFile", &(h, "a", "", "Save", options.clone()))
            .await
            .unwrap();
        let h = ObjectPath::try_from("/r/f3").unwrap();
        let _: (u32, HashMap<String, OwnedValue>) = p
            .call("SaveFiles", &(h, "a", "", "Save many", options))
            .await
            .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].1.kind, KIND_SAVE);
        assert_eq!(seen[0].1.current_name, "report.txt");
        assert_eq!(seen[0].1.current_file, b"/tmp/old.txt");
        assert_eq!(seen[1].1.kind, KIND_SAVE_FILES);
        assert_eq!(seen[1].1.files, vec![b"a.txt".to_vec()]);
    }

    #[tokio::test]
    async fn host_cancel_response_and_error() {
        let (_s, c) = setup(MockHost {
            code: 1,
            ..Default::default()
        })
        .await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/f4").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) = p
            .call("OpenFile", &(h, "a", "", "t", Options::new()))
            .await
            .unwrap();
        assert_eq!(code, 1);
        assert!(res.is_empty());

        let (_s2, c2) = setup(MockHost {
            fail: true,
            ..Default::default()
        })
        .await;
        let p2 = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/f5").unwrap();
        let (code, _): (u32, HashMap<String, OwnedValue>) = p2
            .call("OpenFile", &(h, "a", "", "t", Options::new()))
            .await
            .unwrap();
        assert_eq!(code, 2, "падение хоста не должно ронять демон");
    }

    #[tokio::test]
    async fn request_close_cancels_host() {
        let host = MockHost {
            block: true,
            ..Default::default()
        };
        let cancelled = host.cancelled.clone();
        let seen = host.seen.clone();
        let (_s, c) = setup(host).await;
        let c2 = c.clone();
        let call = tokio::spawn(async move {
            let p = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
            let h = ObjectPath::try_from("/r/f6").unwrap();
            p.call::<_, _, (u32, HashMap<String, OwnedValue>)>(
                "OpenFile",
                &(h, "a", "", "t", Options::new()),
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
        let req = proxy(&c, "/r/f6", "org.freedesktop.impl.portal.Request").await;
        req.call_method("Close", &()).await.unwrap();
        let (code, _) = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code, 1);
        assert_eq!(cancelled.lock().unwrap().as_slice(), ["/r/f6"]);
    }
}
