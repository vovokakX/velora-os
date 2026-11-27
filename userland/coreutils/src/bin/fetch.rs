//! fetch [-o FILE] [-q] URL: downloads URL over HTTP and writes it to
//! standard output or FILE. Follows up to 5 redirects. Only http://
//! (Velora has no TLS for https://).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use coreutils::*;
use velora_http::{Url, UrlError};
use vlibc::fs::File;
use vlibc::net::{self, TcpStream};
use vlibc::{env, eprintln, io};

vlibc::main!(main);

fn usage() -> i32 {
    eprintln!("usage: fetch [-o FILE] [-q] URL");
    2
}

/// One request: the status and the body (or an error message).
fn get(url: &Url) -> Result<(velora_http::Response, alloc::vec::Vec<u8>), String> {
    let ip =
        net::resolve(&url.host).map_err(|e| alloc::format!("{}: {}", url.host, e.describe()))?;
    let mut s = TcpStream::connect(ip, url.port)
        .map_err(|e| alloc::format!("{}:{}: {}", ip, url.port, describe(e)))?;
    s.write_all(velora_http::get_request(url, "Velora-fetch/1").as_bytes())
        .map_err(|e| alloc::format!("send: {}", describe(e)))?;
    let raw = s
        .read_to_end()
        .map_err(|e| alloc::format!("receive: {}", describe(e)))?;
    let (resp, at) =
        velora_http::parse_head(&raw).map_err(|_| String::from("not an HTTP response"))?;
    let body = velora_http::body(&resp, &raw[at..])
        .map_err(|_| String::from("the response body is cut short"))?;
    Ok((resp, body))
}

fn main() -> i32 {
    let args = env::args();
    let mut out = None;
    let mut quiet = false;
    let mut target = None;
    let mut i = 1;
    while let Some(a) = args.get(i) {
        match a {
            "-o" => {
                let Some(f) = args.get(i + 1) else {
                    return usage();
                };
                out = Some(f);
                i += 2;
            }
            "-q" => {
                quiet = true;
                i += 1;
            }
            _ if target.is_none() => {
                target = Some(a);
                i += 1;
            }
            _ => return usage(),
        }
    }
    let Some(target) = target else {
        return usage();
    };
    let mut url = match Url::parse(target) {
        Ok(u) => u,
        Err(UrlError::Unsupported) => {
            eprintln!("fetch: {}: only http:// is supported (no TLS)", target);
            return 2;
        }
        Err(UrlError::Invalid) => {
            eprintln!("fetch: {}: not a valid URL", target);
            return 2;
        }
    };
    let mut redirects = 0;
    let (resp, body) = loop {
        let (resp, body) = match get(&url) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("fetch: {}", e);
                return 1;
            }
        };
        if resp.is_redirect() && redirects < 5 {
            let Some(loc) = resp.header("location") else {
                break (resp, body);
            };
            url = match url.join(loc) {
                Ok(u) => u,
                Err(_) => {
                    eprintln!("fetch: redirect to an unsupported URL: {}", loc);
                    return 1;
                }
            };
            redirects += 1;
            continue;
        }
        break (resp, body);
    };
    if !(200..300).contains(&resp.status) {
        eprintln!("fetch: HTTP {} {}", resp.status, resp.reason);
        return 1;
    }
    let written = match out {
        Some(path) => File::create(path).and_then(|mut f| f.write_all(&body)),
        None => io::write_all(1, &body),
    };
    if let Err(e) = written {
        return fail(out.unwrap_or("standard output"), e);
    }
    if !quiet {
        eprintln!(
            "fetch: {} bytes from {}{}",
            body.len(),
            url.host_header(),
            url.path
        );
    }
    0
}
