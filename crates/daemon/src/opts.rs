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
            warn!(
                key,
                "ожидалась строка, получено {}",
                other.value_signature()
            );
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
        o.insert(
            "modal".into(),
            OwnedValue::try_from(Value::from("yes")).unwrap(),
        );
        o.insert(
            "interactive".into(),
            OwnedValue::try_from(Value::from(true)).unwrap(),
        );
        o.insert(
            "target".into(),
            OwnedValue::try_from(Value::from(4u32)).unwrap(),
        );
        assert_eq!(get_bool(&o, "modal"), None);
        assert_eq!(get_bool(&o, "interactive"), Some(true));
        assert_eq!(get_u32(&o, "target"), Some(4));
        assert_eq!(get_u32(&o, "missing"), None);
        assert_eq!(get_str(&o, "modal").as_deref(), Some("yes"));
    }
}

// ---- разбор вложенных значений (массивы и структуры портала) ----

use conchy_portal_proto::files::{Choice, ChoiceOption, Filter, FilterRule};

fn array(v: &Value<'_>) -> Option<Vec<Value<'static>>> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| x.try_to_owned().ok().map(Value::from))
            .collect(),
        _ => None,
    }
}

fn fields(v: &Value<'_>) -> Option<Vec<Value<'static>>> {
    match v {
        Value::Structure(s) => s
            .fields()
            .iter()
            .map(|x| x.try_to_owned().ok().map(Value::from))
            .collect(),
        _ => None,
    }
}

fn string(v: &Value<'_>) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// `ay` → байты без завершающего нуля (правило §3 п.4 плана).
pub fn parse_bytes(v: &Value<'_>) -> Option<Vec<u8>> {
    let items = array(v)?;
    let mut out: Vec<u8> = items
        .iter()
        .map(|b| match b {
            Value::U8(n) => Some(*n),
            _ => None,
        })
        .collect::<Option<_>>()?;
    if out.last() == Some(&0) {
        out.pop();
    }
    Some(out)
}

/// `(sa(us))`.
pub fn parse_filter(v: &Value<'_>) -> Option<Filter> {
    let f = fields(v)?;
    let [name, rules] = f.as_slice() else {
        return None;
    };
    let rules = array(rules)?
        .iter()
        .filter_map(|r| {
            let f = fields(r)?;
            let [kind, pat] = f.as_slice() else {
                return None;
            };
            let kind = match kind {
                Value::U32(n) => *n,
                _ => return None,
            };
            Some(FilterRule {
                kind,
                pattern: string(pat)?,
            })
        })
        .collect();
    Some(Filter {
        name: string(name)?,
        rules,
    })
}

/// `a(sa(us))`. Некорректные элементы пропускаются.
pub fn parse_filters(v: &Value<'_>) -> Vec<Filter> {
    array(v)
        .map(|a| a.iter().filter_map(parse_filter).collect())
        .unwrap_or_default()
}

/// `a(ssa(ss)s)`.
pub fn parse_choices(v: &Value<'_>) -> Vec<Choice> {
    array(v)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let f = fields(c)?;
                    let [id, label, opts, initial] = f.as_slice() else {
                        return None;
                    };
                    let options = array(opts)?
                        .iter()
                        .filter_map(|o| {
                            let f = fields(o)?;
                            let [id, label] = f.as_slice() else {
                                return None;
                            };
                            Some(ChoiceOption {
                                id: string(id)?,
                                label: string(label)?,
                            })
                        })
                        .collect();
                    Some(Choice {
                        id: string(id)?,
                        label: string(label)?,
                        options,
                        initial: string(initial)?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `aay` → список путей без нулей.
pub fn parse_byte_arrays(v: &Value<'_>) -> Vec<Vec<u8>> {
    array(v)
        .map(|a| a.iter().filter_map(parse_bytes).collect())
        .unwrap_or_default()
}

pub fn get_value<'a>(o: &'a Options, key: &str) -> Option<&'a Value<'static>> {
    o.get(key).map(|v| &**v)
}

#[cfg(test)]
mod nested_tests {
    use super::*;

    #[test]
    fn parses_filters_choices_bytes() {
        let filters = Value::from(vec![
            (
                "Images".to_owned(),
                vec![(0u32, "*.png".to_owned()), (1u32, "image/jpeg".to_owned())],
            ),
            ("All".to_owned(), vec![(0u32, "*".to_owned())]),
        ]);
        let fs = parse_filters(&filters);
        assert_eq!(fs.len(), 2);
        assert_eq!(fs[0].name, "Images");
        assert_eq!(
            fs[0].rules[1],
            FilterRule {
                kind: 1,
                pattern: "image/jpeg".into()
            }
        );

        let one = Value::from(("Docs".to_owned(), vec![(0u32, "*.pdf".to_owned())]));
        assert_eq!(parse_filter(&one).unwrap().name, "Docs");

        let choices = Value::from(vec![(
            "enc".to_owned(),
            "Encoding".to_owned(),
            vec![
                ("utf8".to_owned(), "UTF-8".to_owned()),
                ("koi".to_owned(), "KOI8-R".to_owned()),
            ],
            "utf8".to_owned(),
        )]);
        let cs = parse_choices(&choices);
        assert_eq!(cs[0].options.len(), 2);
        assert_eq!(cs[0].initial, "utf8");

        assert_eq!(
            parse_bytes(&Value::from(b"/home/u\0".to_vec())).unwrap(),
            b"/home/u"
        );
        let files = Value::from(vec![b"/a\0".to_vec(), b"/b\0".to_vec()]);
        assert_eq!(
            parse_byte_arrays(&files),
            vec![b"/a".to_vec(), b"/b".to_vec()]
        );
        // Мусор вместо структуры не роняет разбор.
        assert!(parse_filters(&Value::from("x")).is_empty());
    }
}
