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
            break Err(AuthError::FlowCancelled(
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
            origin = origin_from_header(&line).unwrap_or(origin);
        }

        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_ascii_uppercase();
        let target = parts.next().unwrap_or("");
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();

        let cors = cors_headers((!origin.is_empty()).then_some(origin.as_str()));

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

        let body = callback_html(params.contains_key("code"));
        let response = format!(
            "HTTP/1.1 200 OK\r\n{cors}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        );
        write_http_response(&mut stream, &response);

        if let Some(error) = params.get("error") {
            let desc = params
                .get("error_description")
                .cloned()
                .unwrap_or_else(|| error.clone());
            break Err(AuthError::Provider(desc));
        }
        let code = params.get("code").cloned().unwrap_or_default();
        let state = params
            .get("state")
            .cloned()
            .ok_or_else(|| AuthError::Provider("callback missing `state`".to_string()))?;
        if state != expected_state {
            break Err(AuthError::Provider(
                "state mismatch on OAuth callback (possible CSRF); aborting".to_string(),
            ));
        }
        break Ok(CallbackResult { code, state });
    }
}

fn callback_html(has_code: bool) -> &'static str {
    if has_code {
        CALLBACK_SUCCESS_HTML
    } else {
        CALLBACK_FAILED_HTML
    }
}

const CALLBACK_SUCCESS_HTML: &str = concat!(
    "<!DOCTYPE html>\n",
    "<html lang=\"en\">\n",
    "<head>\n",
    "<meta charset=\"utf-8\">\n",
    "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
    "<meta name=\"color-scheme\" content=\"only dark\">\n",
    "<meta name=\"theme-color\" content=\"#0a0a0a\">\n",
    "<title>whycodes login complete</title>\n",
    "<style>\n",
    include_str!("callback.css"),
    "</style>\n",
    "</head>\n",
    "<body class=\"ok\">\n",
    include_str!("callback_success.html"),
    "</body>\n",
    "</html>\n",
);

const CALLBACK_FAILED_HTML: &str = concat!(
    "<!DOCTYPE html>\n",
    "<html lang=\"en\">\n",
    "<head>\n",
    "<meta charset=\"utf-8\">\n",
    "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
    "<meta name=\"color-scheme\" content=\"only dark\">\n",
    "<meta name=\"theme-color\" content=\"#0a0a0a\">\n",
    "<title>whycodes login failed</title>\n",
    "<style>\n",
    include_str!("callback.css"),
    "</style>\n",
    "</head>\n",
    "<body class=\"err\">\n",
    include_str!("callback_failed.html"),
    "</body>\n",
    "</html>\n",
);

fn origin_from_header(line: &str) -> Option<String> {
    let (name, value) = line.split_once(':')?;
    if !name.eq_ignore_ascii_case("origin") {
        return None;
    }
    Some(value.trim().to_string())
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
    extra: &[(String, String)],
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
    pairs.extend(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!("{base}?{query}")
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod tests;
