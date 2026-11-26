//! `org.freedesktop.impl.portal.Screenshot` (version 3): Screenshot и PickColor.
//!
//! * неинтерактивный снимок — `org.gnome.Shell.Screenshot` в conchy-wm (файл пишет wm в
//!   настроенный каталог скриншотов);
//! * интерактивный/по области — `conchy-shot area --portal-output <файл>`;
//! * PickColor — `org.gnome.Shell.Screenshot.PickColor` в conchy-wm.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use conchy_portal_proto::response;
use tracing::{info, warn};
use zbus::{
    Connection, ObjectServer, interface, proxy,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

use crate::opts::{Options, get_bool, get_u32};
use crate::request::RequestGuard;
use crate::uri::path_to_uri;

pub const VERSION: u32 = 3;
const TARGET_SCREEN: u32 = 1;
const TARGET_AREA: u32 = 4;
/// Какие цели умеем: весь экран и область. Окно/активное окно — нет.
pub const AVAILABLE_TARGETS: u32 = TARGET_SCREEN | TARGET_AREA;

type Results = HashMap<String, Value<'static>>;

#[proxy(
    interface = "org.gnome.Shell.Screenshot",
    default_service = "org.gnome.Shell.Screenshot",
    default_path = "/org/gnome/Shell/Screenshot"
)]
trait ShellScreenshot {
    fn screenshot(
        &self,
        include_cursor: bool,
        flash: bool,
        filename: &str,
    ) -> zbus::Result<(bool, String)>;
    fn pick_color(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

pub struct Screenshot {
    /// Исполняемый файл интерактивного скриншотера (`CONCHY_SHOT_BIN`, по умолчанию `conchy-shot`).
    shot_bin: PathBuf,
    /// Куда класть файлы интерактивных снимков.
    out_dir: PathBuf,
    counter: AtomicU64,
}

impl Screenshot {
    pub fn new(shot_bin: PathBuf, out_dir: PathBuf) -> Self {
        Self {
            shot_bin,
            out_dir,
            counter: AtomicU64::new(0),
        }
    }

    pub fn from_env() -> Self {
        let shot_bin = std::env::var_os("CONCHY_SHOT_BIN")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from("conchy-shot"), PathBuf::from);
        let runtime =
            std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        Self::new(shot_bin, runtime.join("conchy-portal"))
    }

    fn fail(why: impl std::fmt::Display) -> (u32, Results) {
        warn!("Screenshot: {why}");
        (response::OTHER, Results::new())
    }

    /// Неинтерактивный снимок всего экрана через conchy-wm.
    async fn take_screen(conn: &Connection) -> (u32, Results) {
        let wm = match ShellScreenshotProxy::new(conn).await {
            Ok(p) => p,
            Err(e) => return Self::fail(format!("нет прокси к conchy-wm: {e}")),
        };
        match wm.screenshot(false, false, "").await {
            Ok((true, path)) => uri_result(&PathBuf::from(path)),
            Ok((false, _)) => Self::fail("conchy-wm не сделал снимок"),
            Err(e) => Self::fail(format!("conchy-wm: {e}")),
        }
    }

    /// Интерактивный выбор области через `conchy-shot`. Отмена (`Request.Close`) убивает процесс:
    /// `kill_on_drop` срабатывает при сбросе future в `select!`.
    async fn take_area(&self, id: u64) -> (u32, Results) {
        if let Err(e) = std::fs::create_dir_all(&self.out_dir) {
            return Self::fail(format!("не создать {}: {e}", self.out_dir.display()));
        }
        let out = self
            .out_dir
            .join(format!("screenshot-{}-{id}.png", std::process::id()));
        let status = tokio::process::Command::new(&self.shot_bin)
            .arg("area")
            .arg("--portal-output")
            .arg(&out)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await;
        match status {
            Ok(s) if s.success() && out.exists() => uri_result(&out),
            Ok(s) if s.code() == Some(1) => (response::CANCELLED, Results::new()),
            Ok(s) => Self::fail(format!("{} завершился: {s}", self.shot_bin.display())),
            Err(e) => Self::fail(format!("не запустить {}: {e}", self.shot_bin.display())),
        }
    }
}

fn uri_result(path: &std::path::Path) -> (u32, Results) {
    let mut r = Results::new();
    r.insert("uri".into(), Value::from(path_to_uri(path)));
    (response::OK, r)
}

#[interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl Screenshot {
    async fn screenshot(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        parent_window: String,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let started = Instant::now();
        let handle = OwnedObjectPath::from(handle);
        let (_guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => return Self::fail(e),
        };
        let interactive = get_bool(&options, "interactive").unwrap_or(false);
        let target = get_u32(&options, "target");
        info!(%handle, app_id, parent_window, interactive, ?target, "Screenshot");

        // Пока нет Access-диалога (Фаза 5), разрешение считается выданным.
        let area = interactive || target == Some(TARGET_AREA);
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        let work = async {
            if area {
                self.take_area(id).await
            } else {
                Self::take_screen(conn).await
            }
        };
        let out = tokio::select! {
            r = work => r,
            _ = cancel => (response::CANCELLED, Results::new()),
        };
        info!(%handle, response = out.0, ms = started.elapsed().as_secs_f64() * 1000.0, "Screenshot готов");
        out
    }

    async fn pick_color(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        _parent_window: String,
        _options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let handle = OwnedObjectPath::from(handle);
        let (_guard, cancel) = match RequestGuard::new(server, handle.clone()).await {
            Ok(g) => g,
            Err(e) => return Self::fail(e),
        };
        info!(%handle, app_id, "PickColor");
        let work = async {
            let wm = match ShellScreenshotProxy::new(conn).await {
                Ok(p) => p,
                Err(e) => return Self::fail(format!("нет прокси к conchy-wm: {e}")),
            };
            match wm.pick_color().await {
                Ok(mut m) => match m.remove("color") {
                    Some(c) => {
                        let mut r = Results::new();
                        r.insert("color".into(), c.into());
                        (response::OK, r)
                    }
                    None => Self::fail("wm не вернул color"),
                },
                // Пользователь прервал выбор — conchy-wm отвечает ошибкой «no color picked».
                Err(zbus::Error::MethodError(_, Some(msg), _))
                    if msg.contains("no color picked") =>
                {
                    (response::CANCELLED, Results::new())
                }
                Err(e) => Self::fail(format!("conchy-wm: {e}")),
            }
        };
        tokio::select! {
            r = work => r,
            _ = cancel => (response::CANCELLED, Results::new()),
        }
    }

    #[zbus(property, name = "AvailableTargets")]
    fn available_targets(&self) -> u32 {
        AVAILABLE_TARGETS
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
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    const IFACE: &str = "org.freedesktop.impl.portal.Screenshot";

    /// Мок `org.gnome.Shell.Screenshot` (живёт на клиентской стороне p2p).
    struct MockWm {
        path: String,
        color: Option<(f64, f64, f64)>,
        fail: bool,
    }

    #[interface(name = "org.gnome.Shell.Screenshot")]
    impl MockWm {
        async fn screenshot(
            &self,
            _c: bool,
            _f: bool,
            _n: &str,
        ) -> zbus::fdo::Result<(bool, String)> {
            if self.fail {
                return Err(zbus::fdo::Error::Failed("internal error".into()));
            }
            Ok((true, self.path.clone()))
        }
        async fn pick_color(&self) -> zbus::fdo::Result<HashMap<String, OwnedValue>> {
            let Some(c) = self.color else {
                return Err(zbus::fdo::Error::Failed("no color picked".into()));
            };
            let mut m = HashMap::new();
            m.insert(
                "color".to_owned(),
                OwnedValue::try_from(Value::from(c)).unwrap(),
            );
            Ok(m)
        }
    }

    fn fake_shot(dir: &std::path::Path, body: &str) -> PathBuf {
        let p = dir.join("fake-shot.sh");
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = --portal-output ] && out=\"$2\"; shift; done\n{body}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    async fn setup(shot: PathBuf, out_dir: PathBuf, wm: MockWm) -> (Connection, Connection) {
        let (server, client) = p2p_pair().await;
        server
            .object_server()
            .at(
                conchy_portal_proto::PORTAL_OBJECT_PATH,
                Screenshot::new(shot, out_dir),
            )
            .await
            .unwrap();
        client
            .object_server()
            .at("/org/gnome/Shell/Screenshot", wm)
            .await
            .unwrap();
        (server, client)
    }

    fn opts(interactive: bool) -> Options {
        let mut o = Options::new();
        o.insert(
            "interactive".into(),
            OwnedValue::try_from(Value::from(interactive)).unwrap(),
        );
        o
    }

    async fn call_shot(
        client: &Connection,
        handle: &str,
        o: Options,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let p = proxy(client, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from(handle).unwrap();
        p.call("Screenshot", &(h, "org.test.App", "", o))
            .await
            .unwrap()
    }

    fn mock(path: &str) -> MockWm {
        MockWm {
            path: path.into(),
            color: Some((0.1, 0.2, 0.3)),
            fail: false,
        }
    }

    #[tokio::test]
    async fn non_interactive_uses_wm_path() {
        let d = tempfile::tempdir().unwrap();
        let (_s, c) = setup(
            PathBuf::from("/nonexistent"),
            d.path().into(),
            mock("/home/u/Pictures/Screenshots/a b.png"),
        )
        .await;
        let (code, res) = call_shot(&c, "/r/1", opts(false)).await;
        assert_eq!(code, 0);
        let uri: String = res["uri"].clone().try_into().unwrap();
        assert_eq!(uri, "file:///home/u/Pictures/Screenshots/a%20b.png");
    }

    #[tokio::test]
    async fn wm_failure_is_response_2() {
        let d = tempfile::tempdir().unwrap();
        let wm = MockWm {
            path: String::new(),
            color: None,
            fail: true,
        };
        let (_s, c) = setup(PathBuf::from("/nonexistent"), d.path().into(), wm).await;
        assert_eq!(call_shot(&c, "/r/2", opts(false)).await.0, 2);
    }

    #[tokio::test]
    async fn interactive_runs_shot_and_returns_file() {
        let d = tempfile::tempdir().unwrap();
        let shot = fake_shot(d.path(), "printf PNG > \"$out\"");
        let (_s, c) = setup(shot, d.path().join("out"), mock("")).await;
        let (code, res) = call_shot(&c, "/r/3", opts(true)).await;
        assert_eq!(code, 0);
        let uri: String = res["uri"].clone().try_into().unwrap();
        let p = crate::uri::uri_to_path(&uri).unwrap();
        assert_eq!(std::fs::read(p).unwrap(), b"PNG");
    }

    #[tokio::test]
    async fn interactive_cancel_exit_code_is_response_1() {
        let d = tempfile::tempdir().unwrap();
        let shot = fake_shot(d.path(), "exit 1");
        let (_s, c) = setup(shot, d.path().join("out"), mock("")).await;
        assert_eq!(call_shot(&c, "/r/4", opts(true)).await.0, 1);
    }

    #[tokio::test]
    async fn interactive_crash_is_response_2_and_missing_binary_too() {
        let d = tempfile::tempdir().unwrap();
        let shot = fake_shot(d.path(), "exit 7");
        let (_s, c) = setup(shot, d.path().join("out"), mock("")).await;
        assert_eq!(call_shot(&c, "/r/5", opts(true)).await.0, 2);
        let (_s2, c2) = setup(
            PathBuf::from("/definitely/missing"),
            d.path().join("out"),
            mock(""),
        )
        .await;
        assert_eq!(call_shot(&c2, "/r/6", opts(true)).await.0, 2);
    }

    #[tokio::test]
    async fn request_close_kills_shot() {
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("pid");
        let shot = fake_shot(
            d.path(),
            &format!("echo $$ > {}; exec sleep 30", pidfile.display()),
        );
        let (_s, c) = setup(shot, d.path().join("out"), mock("")).await;

        let c2 = c.clone();
        let call = tokio::spawn(async move { call_shot(&c2, "/r/7", opts(true)).await });
        // Ждём, пока процесс стартует и объект Request появится.
        for _ in 0..100 {
            if pidfile.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let req = proxy(&c, "/r/7", "org.freedesktop.impl.portal.Request").await;
        req.call_method("Close", &()).await.unwrap();
        let (code, _) = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code, 1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "conchy-shot должен быть убит"
        );
    }

    #[tokio::test]
    async fn pick_color_and_cancel() {
        let d = tempfile::tempdir().unwrap();
        let (_s, c) = setup(PathBuf::from("x"), d.path().into(), mock("")).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/8").unwrap();
        let (code, res): (u32, HashMap<String, OwnedValue>) = p
            .call("PickColor", &(h, "a", "", Options::new()))
            .await
            .unwrap();
        assert_eq!(code, 0);
        let rgb: (f64, f64, f64) = res["color"].clone().try_into().unwrap();
        assert_eq!(rgb, (0.1, 0.2, 0.3));

        let wm = MockWm {
            path: String::new(),
            color: None,
            fail: false,
        };
        let (_s2, c2) = setup(PathBuf::from("x"), d.path().into(), wm).await;
        let p2 = proxy(&c2, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        let h = ObjectPath::try_from("/r/9").unwrap();
        let (code, _): (u32, HashMap<String, OwnedValue>) = p2
            .call("PickColor", &(h, "a", "", Options::new()))
            .await
            .unwrap();
        assert_eq!(code, 1);
    }

    #[tokio::test]
    async fn properties() {
        let d = tempfile::tempdir().unwrap();
        let (_s, c) = setup(PathBuf::from("x"), d.path().into(), mock("")).await;
        let p = proxy(&c, conchy_portal_proto::PORTAL_OBJECT_PATH, IFACE).await;
        assert_eq!(p.get_property::<u32>("version").await.unwrap(), 3);
        assert_eq!(p.get_property::<u32>("AvailableTargets").await.unwrap(), 5);
    }
}
