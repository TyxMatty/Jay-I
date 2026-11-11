//! HTTPS GET with WinHTTP, Windows' own HTTP and TLS: the proxy configured in Windows (fixed,
//! PAC or auto-detected) and the certificates Windows trusts, so it also works behind a company
//! proxy that inspects TLS with a root installed on the machine. A Rust client (ureq + rustls)
//! used neither. Came from Jay-C's updater (0.4.0 to 0.5.2).

use std::io::{self, Read};

/// "https://host/path" -> ("host", "/path"). Only https, no user or port.
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/').unwrap_or(rest.len());
    let (host, path) = rest.split_at(slash);
    (!host.is_empty() && !host.contains([':', '@'])).then_some((host, if path.is_empty() { "/" } else { path }))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{Error, HSTRING, PCWSTR, w};

    /// Closes a WinHTTP handle when dropped.
    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                let _ = unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    /// WinHTTP's own errors (12xxx) have no text in the system table: the common ones are named
    /// here, and the code always goes along.
    fn text(what: &str, e: Error) -> String {
        let code = e.code().0 as u32;
        let known = match code & 0xFFFF {
            12002 => "timed out",
            12007 => "the name was not found",
            12029 | 12030 => "could not connect",
            12175 => "the certificate was not accepted",
            _ => "",
        };
        let message = if known.is_empty() { e.message() } else { known.to_owned() };
        format!("{what}: {message} ({code:#010x})")
    }

    fn open(handle: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
        if handle.is_null() { Err(text(what, Error::from_thread())) } else { Ok(Handle(handle)) }
    }

    /// A response being read. Fields drop in this order: request, connection, session.
    pub struct Response {
        pub status: u32,
        request: Handle,
        _connect: Handle,
        _session: Handle,
    }

    impl Read for Response {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut read = 0u32;
            let len = buf.len().min(u32::MAX as usize) as u32;
            unsafe { WinHttpReadData(self.request.0, buf.as_mut_ptr().cast(), len, &mut read) }
                .map_err(|e| io::Error::other(text("reading", e)))?;
            Ok(read as usize)
        }
    }

    /// `proxy` ("host:port") is for tests; None is Windows' own proxy settings.
    pub fn get_via(url: &str, user_agent: &str, proxy: Option<&str>) -> Result<Response, String> {
        let (host, path) = split_url(url).ok_or_else(|| format!("not an https address: {url}"))?;
        unsafe {
            let session = match proxy {
                None => WinHttpOpen(&HSTRING::from(user_agent), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
                Some(proxy) => WinHttpOpen(&HSTRING::from(user_agent), WINHTTP_ACCESS_TYPE_NAMED_PROXY, &HSTRING::from(proxy), PCWSTR::null(), 0),
            };
            let session = open(session, "WinHttpOpen")?;
            // Resolve, connect, send, receive (ms): a stalled network fails instead of hanging.
            WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000).map_err(|e| text("timeouts", e))?;
            let connect = open(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0), "WinHttpConnect")?;
            // Secure: TLS checked by Windows. A redirect from https to http is refused (WinHTTP's default).
            let request = open(
                WinHttpOpenRequest(connect.0, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE),
                "WinHttpOpenRequest",
            )?;
            let headers: Vec<u16> = "Accept: application/vnd.github+json, application/octet-stream".encode_utf16().collect();
            WinHttpAddRequestHeaders(request.0, &headers, WINHTTP_ADDREQ_FLAG_ADD).map_err(|e| text("headers", e))?;
            WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| text(&format!("connecting to {host}"), e))?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| text(&format!("waiting for {host}"), e))?;
            let mut status = 0u32;
            let mut size = 4u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(|e| text("status", e))?;
            Ok(Response { status, request, _connect: connect, _session: session })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct Response {
        pub status: u32,
    }

    impl Read for Response {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    pub fn get_via(url: &str, _: &str, _: Option<&str>) -> Result<Response, String> {
        split_url(url).ok_or_else(|| format!("not an https address: {url}"))?;
        Err("downloads only work on Windows".into())
    }
}

pub use imp::Response;

/// One GET (redirects followed, https only): the status and the body to read.
pub fn get(url: &str, user_agent: &str) -> Result<Response, String> {
    imp::get_via(url, user_agent, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_https_addresses() {
        assert_eq!(split_url("https://api.github.com/repos/x"), Some(("api.github.com", "/repos/x")));
        assert_eq!(split_url("https://github.com"), Some(("github.com", "/")));
        assert_eq!(split_url("http://github.com/x"), None, "only https");
        assert_eq!(split_url("https://user@evil.com/x"), None);
        assert_eq!(split_url("https://github.com:8443/x"), None);
        assert!(get("http://github.com/x", "Jay-I test").err().unwrap().contains("not an https address"));
    }

    /// Through a local test proxy: `JAY_I_TEST_PROXY=127.0.0.1:18081 cargo test via_a_proxy -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn via_a_proxy() {
        let proxy = std::env::var("JAY_I_TEST_PROXY").expect("JAY_I_TEST_PROXY");
        match imp::get_via("https://api.github.com/repos/TyxMatty/Jay-C/releases/latest", "Jay-I test", Some(&proxy)) {
            Ok(mut response) => {
                let mut body = Vec::new();
                response.read_to_end(&mut body).unwrap();
                println!("status {}, {} bytes", response.status, body.len());
            }
            Err(e) => println!("error: {e}"),
        }
    }
}
