//! `file://` URI ↔ путь. Пути — байтовые (`OsStr`), не `String`.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/')
}

/// `/tmp/a b.png` → `file:///tmp/a%20b.png`.
pub fn path_to_uri(p: &Path) -> String {
    let mut out = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `file:///tmp/a%20b.png` → `/tmp/a b.png`. Не `file://` (или с хостом, кроме `localhost`) — `None`.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for s in ["/tmp/a b.png", "/home/м/файл #1.txt", "/a/b%c", "/x/y"] {
            let p = Path::new(s);
            assert_eq!(uri_to_path(&path_to_uri(p)).as_deref(), Some(p), "{s}");
        }
        assert_eq!(path_to_uri(Path::new("/tmp/a b")), "file:///tmp/a%20b");
    }

    #[test]
    fn non_utf8_roundtrip() {
        let p = PathBuf::from(OsString::from_vec(b"/tmp/\xff\xfe".to_vec()));
        assert_eq!(uri_to_path(&path_to_uri(&p)), Some(p));
    }

    #[test]
    fn rejects_foreign() {
        assert_eq!(uri_to_path("http://x/y"), None);
        assert_eq!(uri_to_path("file://host/x"), None);
        assert_eq!(uri_to_path("file:///x%zz"), None);
        assert_eq!(uri_to_path("file://localhost/x"), Some(PathBuf::from("/x")));
    }
}
