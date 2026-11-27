//! HTTP/1.x client logic without I/O, for `fetch`: URL parsing, the
//! request, the response head and the body (whole, by Content-Length or
//! chunked). Only `http://`; `https://` needs TLS, which Velora does not
//! have.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Path with the query, starting with '/'.
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    /// `https://` and other schemes.
    Unsupported,
    Invalid,
}

impl Url {
    pub fn parse(s: &str) -> Result<Url, UrlError> {
        let rest = if let Some(r) = s.strip_prefix("http://") {
            r
        } else if s.contains("://") {
            return Err(UrlError::Unsupported);
        } else {
            s
        };
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let path = if path.starts_with('?') {
            format!("/{}", path)
        } else {
            path.to_string()
        };
        let path = path.split('#').next().unwrap_or("/").to_string();
        let authority = authority.rsplit('@').next().unwrap_or("");
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| UrlError::Invalid)?),
            None => (authority, 80),
        };
        if host.is_empty() || port == 0 || host.contains(|c: char| c.is_whitespace()) {
            return Err(UrlError::Invalid);
        }
        Ok(Url {
            host: host.to_string(),
            port,
            path,
        })
    }

    /// Resolves a Location header against this URL.
    pub fn join(&self, location: &str) -> Result<Url, UrlError> {
        if location.contains("://") {
            return Url::parse(location);
        }
        let path = if location.starts_with('/') {
            location.to_string()
        } else {
            let dir = &self.path[..self.path.rfind('/').map_or(0, |i| i + 1)];
            format!("{}{}", dir, location)
        };
        Ok(Url {
            host: self.host.clone(),
            port: self.port,
            path,
        })
    }

    /// The Host header value.
    pub fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// A GET request. HTTP/1.1 with `Connection: close`: the server closes
/// the connection after the response.
pub fn get_request(url: &Url, agent: &str) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.path,
        url.host_header(),
        agent
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseError {
    /// The head is not complete yet.
    Incomplete,
    Malformed,
    /// The body is cut short.
    Truncated,
}

/// Parses the head of a response. Returns it and where the body starts.
pub fn parse_head(data: &[u8]) -> Result<(Response, usize), ResponseError> {
    let end = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(ResponseError::Incomplete)?;
    let head = core::str::from_utf8(&data[..end]).map_err(|_| ResponseError::Malformed)?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(ResponseError::Malformed)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(ResponseError::Malformed);
    }
    let status = parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(ResponseError::Malformed)?;
    let reason = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    for l in lines {
        let (n, v) = l.split_once(':').ok_or(ResponseError::Malformed)?;
        headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    Ok((
        Response {
            status,
            reason,
            headers,
        },
        end + 4,
    ))
}

/// The body of a whole response (everything after the head, received
/// until the server closed the connection).
pub fn body(resp: &Response, raw: &[u8]) -> Result<Vec<u8>, ResponseError> {
    if resp
        .header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        return dechunk(raw);
    }
    if let Some(len) = resp.header("content-length") {
        let len: usize = len.parse().map_err(|_| ResponseError::Malformed)?;
        if raw.len() < len {
            return Err(ResponseError::Truncated);
        }
        return Ok(raw[..len].to_vec());
    }
    Ok(raw.to_vec())
}

/// Decodes chunked transfer coding.
pub fn dechunk(mut raw: &[u8]) -> Result<Vec<u8>, ResponseError> {
    let mut out = Vec::new();
    loop {
        let eol = raw
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or(ResponseError::Truncated)?;
        let line = core::str::from_utf8(&raw[..eol]).map_err(|_| ResponseError::Malformed)?;
        let size_hex = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| ResponseError::Malformed)?;
        raw = &raw[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if raw.len() < size + 2 {
            return Err(ResponseError::Truncated);
        }
        out.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = Url::parse("http://example.com/a/b?x=1#frag").unwrap();
        assert_eq!(
            u,
            Url {
                host: "example.com".into(),
                port: 80,
                path: "/a/b?x=1".into()
            }
        );
        let u = Url::parse("10.0.2.2:8080").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.path.as_str()),
            ("10.0.2.2", 8080, "/")
        );
        assert_eq!(u.host_header(), "10.0.2.2:8080");
        assert_eq!(Url::parse("http://h?q").unwrap().path, "/?q");
        assert_eq!(
            Url::parse("https://example.com/"),
            Err(UrlError::Unsupported)
        );
        assert_eq!(Url::parse("http://:80/"), Err(UrlError::Invalid));
        assert_eq!(Url::parse("http://h:99999/"), Err(UrlError::Invalid));
    }

    #[test]
    fn redirects() {
        let u = Url::parse("http://h:81/dir/page").unwrap();
        assert_eq!(u.join("/other").unwrap().path, "/other");
        assert_eq!(u.join("next").unwrap().path, "/dir/next");
        assert_eq!(u.join("http://x/y").unwrap().host, "x");
        assert_eq!(u.join("http://x/y").unwrap().port, 80);
    }

    #[test]
    fn request_text() {
        let u = Url::parse("http://velora.test/f.txt").unwrap();
        assert_eq!(
            get_request(&u, "fetch"),
            "GET /f.txt HTTP/1.1\r\nHost: velora.test\r\nUser-Agent: fetch\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn responses() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-A: b\r\n\r\nhello world";
        let (r, at) = parse_head(raw).unwrap();
        assert_eq!((r.status, r.reason.as_str()), (200, "OK"));
        assert_eq!(r.header("x-a"), Some("b"));
        assert_eq!(body(&r, &raw[at..]).unwrap(), b"hello");
        assert_eq!(
            parse_head(b"HTTP/1.1 200 OK\r\n"),
            Err(ResponseError::Incomplete)
        );
        assert_eq!(
            parse_head(b"SSH-2.0\r\n\r\n"),
            Err(ResponseError::Malformed)
        );
        let (r, at) = parse_head(b"HTTP/1.0 302 Found\r\nLocation: /x\r\n\r\n").unwrap();
        assert!(r.is_redirect());
        assert_eq!(at, 36);
        let (r, _) = parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n").unwrap();
        assert_eq!(body(&r, b"short"), Err(ResponseError::Truncated));
    }

    #[test]
    fn chunked() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n7;ext=1\r\n, world\r\n0\r\n\r\n";
        let (r, at) = parse_head(raw).unwrap();
        assert_eq!(body(&r, &raw[at..]).unwrap(), b"hello, world");
        assert_eq!(dechunk(b"5\r\nhel"), Err(ResponseError::Truncated));
        assert_eq!(dechunk(b"zz\r\n"), Err(ResponseError::Malformed));
    }
}
