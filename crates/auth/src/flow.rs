//! Browser + localhost-callback PKCE helpers and the GitHub device flow.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use crate::error::{AuthError, Result};
use crate::pkce::Pkce;

/// Open the user's browser, falling back to an error that carries the URL so
/// the caller can print it for manual use.
pub fn open_browser(url: &str) -> Result<()> {
    if open::that(url).is_ok() {
        Ok(())
    } else {
        Err(AuthError::BrowserUnavailable(url.to_string()))
    }
}

/// Result of a localhost OAuth callback wait.
pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

/// Wait for the OAuth redirect on `listener`, validate `state`, and answer
/// the browser with a small "you can close this tab" page.
///
/// `timeout` bounds the whole wait so a closed browser tab does not hang the
/// CLI forever.
pub fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<CallbackResult> {
    listener.set_nonblocking(false).map_err(AuthError::Io)?;
    let deadline = std::time::Instant::now() + timeout;
    listener.set_ttl(1).ok(); // best-effort; not all platforms honour this on listeners

    // Accept with a polling loop so the timeout applies to the accept itself.
    listener.set_nonblocking(true).map_err(AuthError::Io)?;
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(pair) => break pair,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    return Err(AuthError::FlowCancelled(
                        "timed out waiting for the browser redirect".to_string(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(AuthError::Io(e)),
        }
    };

    let mut reader = BufReader::new(stream.try_clone().map_err(AuthError::Io)?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(AuthError::Io)?;
    // Drain remaining headers so the browser gets a clean response.
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(AuthError::Io)?;
        if n == 0 || line == "\r\n" {
            break;
        }
    }

    // Extract and parse the query string from `GET /path?query HTTP/1.1`.
    let target = request_line.split_whitespace().nth(1).ok_or_else(|| {
        AuthError::Provider(format!("malformed callback request: {request_line}"))
    })?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

    let body = if params.contains_key("code") {
        "<html><body><h2>whycode login complete</h2><p>You can close this tab and return to the terminal.</p></body></html>"
    } else {
        "<html><body><h2>whycode login failed</h2><p>The provider did not return a code. You can close this tab.</p></body></html>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();

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
    Ok(CallbackResult { code, state })
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
