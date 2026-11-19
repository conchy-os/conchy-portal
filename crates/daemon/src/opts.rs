//! Разбор `a{sv}`: неизвестные ключи игнорируются, неверный тип известного ключа — `warn!`
//! и значение по умолчанию (правило §3 п.8 плана).

use std::collections::HashMap;

use tracing::warn;
use zbus::zvariant::{OwnedValue, Value};

pub type Options = HashMap<String, OwnedValue>;

pub fn get_bool(o: &Options, key: &str) -> Option<bool> {
    let v = o.get(key)?;
    match &**v {
        Value::Bool(b) => Some(*b),
        other => {
            warn!(key, "ожидался bool, получено {}", other.value_signature());
            None
        }
    }
}

pub fn get_u32(o: &Options, key: &str) -> Option<u32> {
    let v = o.get(key)?;
    match &**v {
        Value::U32(n) => Some(*n),
        other => {
            warn!(key, "ожидался u32, получено {}", other.value_signature());
            None
        }
    }
}

pub fn get_str(o: &Options, key: &str) -> Option<String> {
    let v = o.get(key)?;
    match &**v {
        Value::Str(s) => Some(s.to_string()),
        other => {
            warn!(key, "ожидалась строка, получено {}", other.value_signature());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_type_gives_none() {
        let mut o = Options::new();
        o.insert("modal".into(), OwnedValue::try_from(Value::from("yes")).unwrap());
        o.insert("interactive".into(), OwnedValue::try_from(Value::from(true)).unwrap());
        o.insert("target".into(), OwnedValue::try_from(Value::from(4u32)).unwrap());
        assert_eq!(get_bool(&o, "modal"), None);
        assert_eq!(get_bool(&o, "interactive"), Some(true));
        assert_eq!(get_u32(&o, "target"), Some(4));
        assert_eq!(get_u32(&o, "missing"), None);
        assert_eq!(get_str(&o, "modal").as_deref(), Some("yes"));
    }
}
