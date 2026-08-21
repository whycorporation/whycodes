//! Browser + localhost-callback PKCE helpers and the GitHub device flow.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use crate::error::{AuthError, Result};
use crate::pkce::Pkce;

/// Open the user's browser, falling back to an error that carries the URL so
/// the caller can print it for manual use.
pub fn open_browser(url: &str) -> Result<()> {
    open_browser_with(url, |target| open::that(target))
}

fn open_browser_with(url: &str, opener: impl FnOnce(&str) -> std::io::Result<()>) -> Result<()> {
    if opener(url).is_ok() {
        Ok(())
    } else {
        Err(AuthError::BrowserUnavailable(url.to_string()))
    }
}

/// Result of a localhost OAuth callback wait.
#[derive(Debug)]
pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

/// Origins the xAI accounts app uses when it fetches the loopback
/// callback (Private Network Access). Other providers navigate here with
/// a 302, so extra CORS headers on those responses are unused.
const ACCOUNTS_APP_ORIGINS: &[&str] = &["https://accounts.x.ai", "https://auth.x.ai"];

/// Wait for the OAuth redirect on `listener`, validate `state`, and answer
/// the browser with a small "you can close this tab" page.
///
/// `timeout` bounds the whole wait so a closed browser tab does not hang the
/// CLI forever. OPTIONS preflights, favicon fetches, and other requests
/// without `code`/`error` are answered and ignored so the real callback
/// can still land (xAI's accounts app hits loopback via CORS/PNA).
pub fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<CallbackResult> {
    let deadline = std::time::Instant::now() + timeout;
    listener.set_ttl(1).ok(); // best-effort; not all platforms honour this on listeners
    listener.set_nonblocking(true).map_err(AuthError::Io)?;

    loop {
        if std::time::Instant::now() > deadline {
            return Err(AuthError::FlowCancelled(
                "timed out waiting for the browser redirect".to_string(),
            ));
        }
        let Some((mut stream, _)) = accept_connection(|| listener.accept())? else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };

        let mut reader = BufReader::new(stream.try_clone().map_err(AuthError::Io)?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).map_err(AuthError::Io)?;
        let mut origin = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(AuthError::Io)?;
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("origin")
            {
                origin = value.trim().to_string();
            }
        }

        let method = request_line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        let target = request_line.split_whitespace().nth(1).unwrap_or("");
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();

        let cors = cors_headers(if origin.is_empty() {
            None
        } else {
            Some(origin.as_str())
        });

        if method == "OPTIONS" {
            let response = format!(
                "HTTP/1.1 204 No Content\r\n{cors}Access-Control-Max-Age: 600\r\nConnection: close\r\n\r\n"
            );
            write_http_response(&mut stream, &response);
            continue;
        }

        if !params.contains_key("code") && !params.contains_key("error") {
            // Favicon / health check / stray GET — keep waiting.
            let response = format!("HTTP/1.1 204 No Content\r\n{cors}Connection: close\r\n\r\n");
            write_http_response(&mut stream, &response);
            continue;
        }

        let body = if params.contains_key("code") {
            "<html><body><h2>whycode login complete</h2><p>You can close this tab and return to the terminal.</p></body></html>"
        } else {
            "<html><body><h2>whycode login failed</h2><p>The provider did not return a code. You can close this tab.</p></body></html>"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\n{cors}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        write_http_response(&mut stream, &response);

        if let Some(error) = params.get("error") {
            let desc = params
                .get("error_description")
                .cloned()
                .unwrap_or_else(|| error.clone());
            return Err(AuthError::Provider(desc));
        }
        let code = params
            .get("code")
            .cloned()
            .ok_or_else(|| AuthError::Provider("callback missing `code`".to_string()))?;
        let state = params
            .get("state")
            .cloned()
            .ok_or_else(|| AuthError::Provider("callback missing `state`".to_string()))?;
        if state != expected_state {
            return Err(AuthError::Provider(
                "state mismatch on OAuth callback (possible CSRF); aborting".to_string(),
            ));
        }
        return Ok(CallbackResult { code, state });
    }
}

fn accept_connection(
    accept: impl FnOnce() -> std::io::Result<(std::net::TcpStream, std::net::SocketAddr)>,
) -> Result<Option<(std::net::TcpStream, std::net::SocketAddr)>> {
    match accept() {
        Ok(pair) => Ok(Some(pair)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(AuthError::Io(error)),
    }
}

fn write_http_response(stream: &mut impl Write, response: &str) {
    let _write_failed = stream.write_all(response.as_bytes()).is_err();
    let _flush_failed = stream.flush().is_err();
}

fn cors_headers(origin: Option<&str>) -> String {
    let allowed = origin
        .filter(|o| ACCOUNTS_APP_ORIGINS.contains(o))
        .unwrap_or("https://accounts.x.ai");
    format!(
        "Access-Control-Allow-Origin: {allowed}\r\n\
         Access-Control-Allow-Methods: GET, OPTIONS\r\n\
         Access-Control-Allow-Headers: *\r\n\
         Access-Control-Allow-Private-Network: true\r\n\
         Vary: Origin\r\n"
    )
}

/// Bind a loopback listener on an ephemeral port for the OAuth redirect.
pub fn bind_loopback() -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(AuthError::Io)?;
    let port = listener.local_addr().map_err(AuthError::Io)?.port();
    Ok((listener, port))
}

/// Build an authorization URL with the common PKCE S256 parameters.
pub fn authorize_url(
    base: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    pkce: &Pkce,
    extra: &[(&str, &str)],
) -> String {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", scope),
        ("state", pkce.state.as_str()),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    pairs.extend_from_slice(extra);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpStream;
    use std::thread;

    fn send(addr: std::net::SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(std::net::Shutdown::Write).ok();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).ok();
        buf
    }

    #[test]
    fn wait_for_callback_skips_options_and_returns_code() {
        let (listener, port) = bind_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let handle =
            thread::spawn(move || wait_for_callback(&listener, "st", Duration::from_secs(5)));

        let preflight = send(
            addr,
            "OPTIONS /callback HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Origin: https://accounts.x.ai\r\n\
             Access-Control-Request-Method: GET\r\n\
             Access-Control-Request-Private-Network: true\r\n\
             \r\n",
        );
        assert!(
            preflight.contains("Access-Control-Allow-Private-Network: true"),
            "{preflight}"
        );
        assert!(preflight.contains("https://accounts.x.ai"), "{preflight}");

        let _ = send(addr, "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

        let page = send(
            addr,
            &format!(
                "GET /callback?code=tok&state=st HTTP/1.1\r\n\
                 Host: 127.0.0.1:{port}\r\n\
                 Origin: https://accounts.x.ai\r\n\
                 \r\n"
            ),
        );
        assert!(page.contains("whycode login complete"), "{page}");

        let result = handle.join().unwrap().unwrap();
        assert_eq!(result.code, "tok");
        assert_eq!(result.state, "st");
    }

    #[test]
    fn wait_for_callback_rejects_state_mismatch() {
        let (listener, _) = bind_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let handle =
            thread::spawn(move || wait_for_callback(&listener, "expected", Duration::from_secs(5)));
        let _ = send(
            addr,
            "GET /callback?code=tok&state=other HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );
        let err = handle.join().unwrap().unwrap_err();
        assert!(err.to_string().contains("state mismatch"), "{}", err);
    }

    #[test]
    fn wait_for_callback_reports_provider_error() {
        let (listener, _) = bind_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let handle =
            thread::spawn(move || wait_for_callback(&listener, "state", Duration::from_secs(5)));
        let page = send(
            addr,
            "GET /callback?error=access_denied&error_description=Nope HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\n\r\n",
        );
        assert!(page.contains("whycode login failed"));
        assert!(page.contains("https://accounts.x.ai"));
        assert_eq!(
            handle.join().unwrap().unwrap_err().to_string(),
            "OAuth provider returned an error: Nope"
        );
    }

    #[test]
    fn wait_for_callback_requires_state() {
        let (listener, _) = bind_loopback().unwrap();
        let addr = listener.local_addr().unwrap();
        let handle =
            thread::spawn(move || wait_for_callback(&listener, "state", Duration::from_secs(5)));
        let _page = send(
            addr,
            "GET /callback?code=tok HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(
            handle
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("missing `state`")
        );
    }

    #[test]
    fn wait_for_callback_times_out() {
        let (listener, _) = bind_loopback().unwrap();
        let err = wait_for_callback(&listener, "state", Duration::ZERO).unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn open_browser_with_maps_opener_result() {
        open_browser_with("https://example.com", |_| Ok(())).unwrap();
        let err = open_browser_with("https://example.com/login", |_| {
            Err(std::io::Error::other("no browser"))
        })
        .unwrap_err();
        assert!(matches!(
            err,
            AuthError::BrowserUnavailable(url) if url == "https://example.com/login"
        ));
    }

    #[test]
    fn accept_connection_surfaces_io_errors() {
        let err = accept_connection(|| Err(std::io::Error::other("accept failed"))).unwrap_err();
        assert!(matches!(err, AuthError::Io(_)));
    }
}
