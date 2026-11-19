//! `org.freedesktop.impl.portal.Settings` (version 2): демон отвечает сам, значения лежат в
//! памяти и обновляются наблюдателем конфигов ([`crate::sources`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use zbus::{interface, object_server::SignalEmitter, zvariant::Value};

pub const NS_APPEARANCE: &str = "org.freedesktop.appearance";
pub const NS_GNOME_INTERFACE: &str = "org.gnome.desktop.interface";

/// Поддерживаемая версия интерфейса.
pub const VERSION: u32 = 2;

/// Значение настройки. Своё перечисление вместо `zvariant::Value`: сравнивается и клонируется
/// без fd-оговорок, а в `Value` превращается в момент ответа.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    U32(u32),
    I32(i32),
    Str(String),
    /// sRGB `(ddd)`, каждый канал `0..=1`.
    Rgb([f64; 3]),
}

impl Val {
    pub fn to_value(&self) -> Value<'static> {
        match self {
            Self::U32(v) => Value::from(*v),
            Self::I32(v) => Value::from(*v),
            Self::Str(v) => Value::from(v.clone()),
            Self::Rgb([r, g, b]) => Value::from((*r, *g, *b)),
        }
    }
}

/// `(namespace, key)` → значение.
pub type Map = BTreeMap<(String, String), Val>;

/// Общее хранилище значений: читает интерфейс, пишет наблюдатель.
#[derive(Clone, Default)]
pub struct Store(Arc<RwLock<Map>>);

impl Store {
    pub fn snapshot(&self) -> Map {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Заменяет содержимое и возвращает изменившиеся/новые ключи (`SettingChanged` — только по ним).
    pub fn replace(&self, new: Map) -> Vec<((String, String), Val)> {
        let Ok(mut g) = self.0.write() else {
            return Vec::new();
        };
        let changed = new
            .iter()
            .filter(|(k, v)| g.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        *g = new;
        changed
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
pub enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
}

pub struct Settings {
    store: Store,
}

impl Settings {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

/// Паттерн namespace: точное имя или префикс с `*` на конце; пустой список = всё.
pub fn ns_matches(patterns: &[String], ns: &str) -> bool {
    patterns.is_empty()
        || patterns.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => ns.starts_with(prefix),
            None => p == ns,
        })
}

#[interface(name = "org.freedesktop.impl.portal.Settings")]
impl Settings {
    async fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, Value<'static>>> {
        let mut out: HashMap<String, HashMap<String, Value<'static>>> = HashMap::new();
        for ((ns, key), val) in self.store.snapshot() {
            if ns_matches(&namespaces, &ns) {
                out.entry(ns).or_default().insert(key, val.to_value());
            }
        }
        out
    }

    async fn read(&self, namespace: String, key: String) -> Result<Value<'static>, PortalError> {
        self.store
            .snapshot()
            .get(&(namespace.clone(), key.clone()))
            .map(Val::to_value)
            .ok_or_else(|| PortalError::NotFound(format!("{namespace}.{key} не найден")))
    }

    #[zbus(signal)]
    pub async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matching() {
        let none: Vec<String> = vec![];
        assert!(ns_matches(&none, "anything"));
        let p = vec!["org.freedesktop.*".to_owned()];
        assert!(ns_matches(&p, "org.freedesktop.appearance"));
        assert!(!ns_matches(&p, "org.gnome.desktop.interface"));
        let p = vec!["org.gnome.desktop.interface".to_owned()];
        assert!(ns_matches(&p, "org.gnome.desktop.interface"));
        assert!(!ns_matches(&p, "org.gnome.desktop.interface.x"));
        let p = vec!["*".to_owned()];
        assert!(ns_matches(&p, "x"));
    }

    #[test]
    fn replace_reports_only_changed_keys() {
        let store = Store::default();
        let k = |n: &str| (NS_APPEARANCE.to_owned(), n.to_owned());
        let mut m = Map::new();
        m.insert(k("color-scheme"), Val::U32(1));
        m.insert(k("contrast"), Val::U32(0));
        assert_eq!(store.replace(m.clone()).len(), 2);
        m.insert(k("color-scheme"), Val::U32(2));
        let ch = store.replace(m);
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].0, k("color-scheme"));
    }
}
