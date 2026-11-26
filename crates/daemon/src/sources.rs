//! Источники значений Settings: конфиги Conchy (`theme.kdl`, `appearance.kdl`, `files.kdl`,
//! `wm.kdl`) и наблюдатель за ними.

use std::path::{Path, PathBuf};
use std::time::Duration;

use conchy_config::schema::appearance::appearance_from_theme_kdl;
use kdl::{KdlDocument, KdlValue};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use zbus::object_server::InterfaceRef;

use crate::settings::{Map, NS_APPEARANCE, NS_GNOME_INTERFACE, Settings, Store, Val};

/// Дебаунс: редакторы и шелл пишут файл несколькими событиями.
const DEBOUNCE: Duration = Duration::from_millis(30);

const WATCHED: [&str; 4] = ["theme.kdl", "appearance.kdl", "files.kdl", "wm.kdl"];

/// Каталог конфигов Conchy: `$XDG_CONFIG_HOME/conchy` или `~/.config/conchy`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/"));
    base.join("conchy")
}

/// Читает файл: `None` — файла нет (это не ошибка), `Some(Err)` — не прочитался.
fn read(dir: &Path, name: &str) -> Option<std::io::Result<String>> {
    match std::fs::read_to_string(dir.join(name)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        r => Some(r),
    }
}

fn key(ns: &str, k: &str) -> (String, String) {
    (ns.to_owned(), k.to_owned())
}

/// Собирает все настройки. Источник, который не удалось разобрать, оставляет **прежние**
/// значения своих ключей (`prev`), а не откатывает их к умолчанию.
pub fn load_all(dir: &Path, prev: &Map) -> Map {
    let mut out = Map::new();
    let keep = |out: &mut Map, ns: &str, keys: &[&str]| {
        for k in keys {
            if let Some(v) = prev.get(&key(ns, k)) {
                out.insert(key(ns, k), v.clone());
            }
        }
    };

    // theme.kdl + appearance.kdl
    const APPEARANCE_KEYS: [&str; 4] =
        ["color-scheme", "accent-color", "contrast", "reduced-motion"];
    let theme = read(dir, "theme.kdl").transpose();
    let overrides = read(dir, "appearance.kdl").transpose();
    let wm_text = read(dir, "wm.kdl").transpose();

    match (theme, overrides) {
        (Ok(theme), Ok(over)) => {
            match appearance_from_theme_kdl(theme.as_deref().unwrap_or(""), over.as_deref()) {
                Ok(a) => {
                    let wm_off = wm_text
                        .as_ref()
                        .ok()
                        .and_then(|t| t.as_deref())
                        .is_some_and(animations_off);
                    out.insert(
                        key(NS_APPEARANCE, "color-scheme"),
                        Val::U32(a.color_scheme.portal_value()),
                    );
                    out.insert(key(NS_APPEARANCE, "accent-color"), Val::Rgb(a.accent));
                    out.insert(key(NS_APPEARANCE, "contrast"), Val::U32(a.contrast));
                    out.insert(
                        key(NS_APPEARANCE, "reduced-motion"),
                        Val::U32(u32::from(a.reduced_motion == 1 || wm_off)),
                    );
                    out.insert(
                        key(NS_GNOME_INTERFACE, "color-scheme"),
                        Val::Str(a.color_scheme.gnome_value().to_owned()),
                    );
                }
                Err(e) => {
                    warn!("theme.kdl/appearance.kdl: {e}; оставляю прежние значения");
                    keep(&mut out, NS_APPEARANCE, &APPEARANCE_KEYS);
                    keep(&mut out, NS_GNOME_INTERFACE, &["color-scheme"]);
                }
            }
        }
        (t, o) => {
            warn!(
                "не прочитать theme.kdl/appearance.kdl: {:?} {:?}",
                t.err(),
                o.err()
            );
            keep(&mut out, NS_APPEARANCE, &APPEARANCE_KEYS);
            keep(&mut out, NS_GNOME_INTERFACE, &["color-scheme"]);
        }
    }

    // files.kdl → icon-theme
    match read(dir, "files.kdl") {
        None => {}
        Some(Ok(text)) => match text
            .parse::<KdlDocument>()
            .map_err(|e| e.to_string())
            .and_then(|d| conchy_config::parse_files_config(&d).map_err(|e| e.to_string()))
        {
            Ok(c) => {
                if let Some(t) = c.icon_theme {
                    out.insert(key(NS_GNOME_INTERFACE, "icon-theme"), Val::Str(t));
                }
            }
            Err(e) => {
                warn!("files.kdl: {e}; оставляю прежнее значение icon-theme");
                keep(&mut out, NS_GNOME_INTERFACE, &["icon-theme"]);
            }
        },
        Some(Err(e)) => {
            warn!("не прочитать files.kdl: {e}");
            keep(&mut out, NS_GNOME_INTERFACE, &["icon-theme"]);
        }
    }

    // wm.kdl → cursor-theme, cursor-size
    match wm_text {
        Ok(None) => {}
        Ok(Some(text)) => match cursor_from_wm_kdl(&text) {
            Ok((theme, size)) => {
                if let Some(t) = theme {
                    out.insert(key(NS_GNOME_INTERFACE, "cursor-theme"), Val::Str(t));
                }
                if let Some(s) = size {
                    out.insert(key(NS_GNOME_INTERFACE, "cursor-size"), Val::I32(s));
                }
            }
            Err(e) => {
                warn!("wm.kdl: {e}; оставляю прежние значения курсора");
                keep(
                    &mut out,
                    NS_GNOME_INTERFACE,
                    &["cursor-theme", "cursor-size"],
                );
            }
        },
        Err(e) => {
            warn!("не прочитать wm.kdl: {e}");
            keep(
                &mut out,
                NS_GNOME_INTERFACE,
                &["cursor-theme", "cursor-size"],
            );
        }
    }
    out
}

fn kdl_str(doc: &KdlDocument, name: &str) -> Option<String> {
    match doc.get_arg(name)? {
        KdlValue::String(s) | KdlValue::RawString(s) => Some(s.clone()),
        _ => None,
    }
}

/// `cursor { xcursor-theme "…"; xcursor-size N }` из `wm.kdl`.
pub fn cursor_from_wm_kdl(text: &str) -> Result<(Option<String>, Option<i32>), String> {
    let doc: KdlDocument = text.parse().map_err(|e: kdl::KdlError| e.to_string())?;
    let Some(block) = doc.get("cursor").and_then(|n| n.children()) else {
        return Ok((None, None));
    };
    let size = match block.get_arg("xcursor-size") {
        Some(KdlValue::Base10(n)) => i32::try_from(*n).ok(),
        _ => None,
    };
    Ok((kdl_str(block, "xcursor-theme"), size))
}

/// Есть ли в `wm.kdl` блок `animations { off }`.
pub fn animations_off(text: &str) -> bool {
    text.parse::<KdlDocument>().ok().is_some_and(|d| {
        d.get("animations")
            .and_then(|n| n.children())
            .is_some_and(|c| c.get("off").is_some())
    })
}

/// Запускает наблюдателя: перечитывает конфиги по изменению файлов и шлёт `SettingChanged`
/// только по изменившимся ключам. Возвращает `Watcher`, который нужно держать живым.
pub fn spawn_watcher(
    dir: PathBuf,
    store: Store,
    iface: InterfaceRef<Settings>,
) -> notify::Result<notify::RecommendedWatcher> {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        let relevant = ev.paths.iter().any(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| WATCHED.contains(&n))
        });
        if relevant {
            let _ = tx.send(());
        }
    })?;
    // Каталог может ещё не существовать — создаём, иначе следить не за чем.
    let _ = std::fs::create_dir_all(&dir);
    // Следим за каталогом: атомарное сохранение (rename) не видно при наблюдении за файлом.
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            tokio::time::sleep(DEBOUNCE).await;
            while rx.try_recv().is_ok() {}
            let span = std::time::Instant::now();
            let new = load_all(&dir, &store.snapshot());
            let changed = store.replace(new);
            for ((ns, k), v) in &changed {
                if let Err(e) =
                    Settings::setting_changed(iface.signal_emitter(), ns, k, v.to_value()).await
                {
                    warn!("SettingChanged {ns}.{k}: {e}");
                }
            }
            if !changed.is_empty() {
                info!(
                    changed = changed.len(),
                    reload_ms = span.elapsed().as_secs_f64() * 1000.0,
                    "настройки обновлены"
                );
            } else {
                debug!("конфиги изменились, значения Settings — нет");
            }
        }
    });
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const DARK: &str = "base16 {\n base00 \"#2e3440ff\"\n base0d \"#81a1c1ff\"\n}\n";
    const LIGHT: &str = "base16 {\n base00 \"#f5f5f5ff\"\n base0d \"#1e66f5ff\"\n}\n";

    fn get(m: &Map, ns: &str, k: &str) -> Option<Val> {
        m.get(&key(ns, k)).cloned()
    }

    #[test]
    fn loads_theme_files_and_wm() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("theme.kdl"), DARK).unwrap();
        fs::write(
            d.path().join("files.kdl"),
            "theme {\n icon-theme \"Papirus\"\n}\n",
        )
        .unwrap();
        fs::write(
            d.path().join("wm.kdl"),
            "cursor {\n xcursor-theme \"Bibata\"\n xcursor-size 32\n}\nanimations {\n off\n}\n",
        )
        .unwrap();
        let m = load_all(d.path(), &Map::new());
        assert_eq!(get(&m, NS_APPEARANCE, "color-scheme"), Some(Val::U32(1)));
        assert_eq!(get(&m, NS_APPEARANCE, "reduced-motion"), Some(Val::U32(1)));
        assert_eq!(
            get(&m, NS_GNOME_INTERFACE, "color-scheme"),
            Some(Val::Str("prefer-dark".into()))
        );
        assert_eq!(
            get(&m, NS_GNOME_INTERFACE, "icon-theme"),
            Some(Val::Str("Papirus".into()))
        );
        assert_eq!(
            get(&m, NS_GNOME_INTERFACE, "cursor-theme"),
            Some(Val::Str("Bibata".into()))
        );
        assert_eq!(
            get(&m, NS_GNOME_INTERFACE, "cursor-size"),
            Some(Val::I32(32))
        );
    }

    #[test]
    fn broken_theme_keeps_previous_values() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("theme.kdl"), DARK).unwrap();
        let first = load_all(d.path(), &Map::new());
        fs::write(d.path().join("theme.kdl"), "base16 {{{ ").unwrap();
        let second = load_all(d.path(), &first);
        assert_eq!(
            get(&second, NS_APPEARANCE, "color-scheme"),
            Some(Val::U32(1))
        );
        assert_eq!(
            get(&second, NS_APPEARANCE, "accent-color"),
            get(&first, NS_APPEARANCE, "accent-color")
        );
    }

    #[test]
    fn missing_files_give_defaults() {
        let d = tempfile::tempdir().unwrap();
        let m = load_all(d.path(), &Map::new());
        assert_eq!(get(&m, NS_APPEARANCE, "color-scheme"), Some(Val::U32(0)));
        assert!(get(&m, NS_GNOME_INTERFACE, "icon-theme").is_none());
    }

    #[tokio::test]
    async fn watcher_emits_setting_changed_fast() {
        use crate::testutil::{p2p_pair, proxy};
        use futures_util::StreamExt;

        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("theme.kdl"), DARK).unwrap();
        let store = Store::default();
        store.replace(load_all(d.path(), &Map::new()));

        let (server, client) = p2p_pair().await;
        server
            .object_server()
            .at(
                conchy_portal_proto::PORTAL_OBJECT_PATH,
                Settings::new(store.clone()),
            )
            .await
            .unwrap();
        let iface = server
            .object_server()
            .interface::<_, Settings>(conchy_portal_proto::PORTAL_OBJECT_PATH)
            .await
            .unwrap();
        let _w = spawn_watcher(d.path().to_path_buf(), store, iface).unwrap();

        let p = proxy(
            &client,
            conchy_portal_proto::PORTAL_OBJECT_PATH,
            "org.freedesktop.impl.portal.Settings",
        )
        .await;
        let mut sig = p.receive_signal("SettingChanged").await.unwrap();

        // ReadAll / Read поверх шины.
        let all: std::collections::HashMap<
            String,
            std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
        > = p
            .call("ReadAll", &(vec!["org.freedesktop.*".to_owned()],))
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        let cs: zbus::zvariant::OwnedValue = p
            .call("Read", &(NS_APPEARANCE, "color-scheme"))
            .await
            .unwrap();
        assert_eq!(u32::try_from(cs).unwrap(), 1);
        assert!(
            p.call_method("Read", &(NS_APPEARANCE, "nope"))
                .await
                .is_err()
        );

        let t0 = std::time::Instant::now();
        fs::write(d.path().join("theme.kdl.tmp"), LIGHT).unwrap();
        fs::rename(d.path().join("theme.kdl.tmp"), d.path().join("theme.kdl")).unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(2), sig.next())
            .await
            .expect("SettingChanged не пришёл")
            .unwrap();
        let (ns, k, _v): (String, String, zbus::zvariant::OwnedValue) =
            msg.body().deserialize().unwrap();
        eprintln!("SettingChanged за {:?}", t0.elapsed());
        assert_eq!(ns, NS_APPEARANCE);
        assert!(t0.elapsed() < Duration::from_millis(500));
        assert!(["color-scheme", "accent-color"].contains(&k.as_str()));
    }
}
