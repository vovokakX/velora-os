//! Path rules: absolute paths with '/' separators, no symbolic links.
//! "." and ".." are resolved lexically by [`normalize`].

use crate::{FsError, Result};
use alloc::string::String;
use velora_abi::{MAX_NAME, MAX_PATH};

/// Checks one file name: not empty, not "." or "..", no '/' or NUL, at
/// most 255 bytes.
pub fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(FsError::Invalid);
    }
    if name.len() > MAX_NAME {
        return Err(FsError::NameTooLong);
    }
    Ok(())
}

/// Turns an absolute path into its normal form: "/" or "/a/b" with no
/// empty, "." or ".." components. ".." at the root stays at the root.
pub fn normalize(path: &str) -> Result<String> {
    if !path.starts_with('/') {
        return Err(FsError::Invalid);
    }
    if path.len() > MAX_PATH {
        return Err(FsError::NameTooLong);
    }
    let mut out = String::with_capacity(path.len());
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                let cut = out.rfind('/').unwrap_or(0);
                out.truncate(cut);
            }
            c => {
                check_name(c)?;
                out.push('/');
                out.push_str(c);
            }
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    Ok(out)
}

/// Splits a normalised path into parent and last name. `None` for "/".
pub fn split_last(norm: &str) -> Option<(&str, &str)> {
    if norm == "/" {
        return None;
    }
    let i = norm.rfind('/')?;
    let parent = if i == 0 { "/" } else { &norm[..i] };
    Some((parent, &norm[i + 1..]))
}

/// True if normalised `path` is `base` or lies below it.
pub fn is_within(path: &str, base: &str) -> bool {
    if base == "/" {
        return true;
    }
    path == base || (path.starts_with(base) && path.as_bytes().get(base.len()) == Some(&b'/'))
}

/// Joins a normalised directory path and a name.
pub fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir);
    if !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(name);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("/").unwrap(), "/");
        assert_eq!(normalize("//a//b/").unwrap(), "/a/b");
        assert_eq!(normalize("/a/./b/../c").unwrap(), "/a/c");
        assert_eq!(normalize("/../..").unwrap(), "/");
        assert_eq!(normalize("/a/b/../../..").unwrap(), "/");
        assert_eq!(normalize("a/b"), Err(FsError::Invalid));
        assert_eq!(normalize(""), Err(FsError::Invalid));
        let long = alloc::format!("/{}", "x".repeat(256));
        assert_eq!(normalize(&long), Err(FsError::NameTooLong));
        assert_eq!(normalize("/a\0b"), Err(FsError::Invalid));
    }

    #[test]
    fn splits_and_checks_names() {
        assert_eq!(split_last("/"), None);
        assert_eq!(split_last("/a"), Some(("/", "a")));
        assert_eq!(split_last("/a/b/c"), Some(("/a/b", "c")));
        assert!(check_name("файл.txt").is_ok());
        assert!(check_name("..").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name(&"y".repeat(255)).is_ok());
    }

    #[test]
    fn within() {
        assert!(is_within("/mnt/a", "/mnt"));
        assert!(is_within("/mnt", "/mnt"));
        assert!(!is_within("/mntx", "/mnt"));
        assert!(is_within("/x", "/"));
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b"), "/a/b");
    }
}
