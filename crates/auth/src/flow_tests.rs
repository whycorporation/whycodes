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
    let handle = thread::spawn(move || wait_for_callback(&listener, "st", Duration::from_secs(5)));

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
    let _ = send(addr, "GET /health HTTP/1.1\nHost: 127.0.0.1\n\n");
    let _ = send(addr, "GET /favicon.ico HTTP/1.1\r\nNotAHeader\r\n\r\n");
    let _ = send(addr, "GET\r\n\r\n");
    let _ = send(addr, "\r\n\r\n");
    // Headers end at EOF rather than a blank line.
    let _ = send(addr, "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n");

    let page = send(
        addr,
        &format!(
            "GET /callback?code=tok&state=st HTTP/1.1\r\n\
                 Host: 127.0.0.1:{port}\r\n\
                 Origin: https://accounts.x.ai\r\n\
                 \r\n"
        ),
    );
    assert!(page.contains("whycodes login complete"), "{page}");

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
    assert!(page.contains("whycodes login failed"));
    assert!(page.contains("https://accounts.x.ai"));
    assert_eq!(
        handle.join().unwrap().unwrap_err().to_string(),
        "OAuth provider returned an error: Nope"
    );
}

#[test]
fn wait_for_callback_error_without_description_uses_error_code() {
    let (listener, _) = bind_loopback().unwrap();
    let addr = listener.local_addr().unwrap();
    let handle =
        thread::spawn(move || wait_for_callback(&listener, "state", Duration::from_secs(5)));
    let _ = send(
        addr,
        "GET /callback?error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    assert_eq!(
        handle.join().unwrap().unwrap_err().to_string(),
        "OAuth provider returned an error: access_denied"
    );
}

#[test]
fn wait_for_callback_error_wins_when_code_is_also_present() {
    let (listener, _) = bind_loopback().unwrap();
    let addr = listener.local_addr().unwrap();
    let handle =
        thread::spawn(move || wait_for_callback(&listener, "state", Duration::from_secs(5)));
    let page = send(
        addr,
        "GET /callback?code=tok&error=access_denied&error_description=Nope HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    assert!(page.contains("whycodes login complete"), "{page}");
    assert_eq!(
        handle.join().unwrap().unwrap_err().to_string(),
        "OAuth provider returned an error: Nope"
    );
}

#[test]
fn open_browser_rejects_an_unopenable_url() {
    let err = open_browser("invalid URL\0").unwrap_err();
    assert!(matches!(err, AuthError::BrowserUnavailable(_)));
}

#[test]
fn write_http_response_swallows_io_errors() {
    struct FailW;
    impl Write for FailW {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fail"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("fail"))
        }
    }
    write_http_response(&mut FailW, "HTTP/1.1 200 OK\r\n\r\n");
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
fn listener_and_port_maps_bind_errors() {
    let err = listener_and_port(Err(std::io::Error::other("bind failed"))).unwrap_err();
    assert!(matches!(err, AuthError::Io(_)));
    let err = port_from_addr(Err(std::io::Error::other("addr failed"))).unwrap_err();
    assert!(matches!(err, AuthError::Io(_)));
    let (listener, port) = bind_loopback().unwrap();
    assert_eq!(port_from_addr(listener.local_addr()).unwrap(), port);
}

#[test]
fn accept_connection_surfaces_io_errors() {
    let err = accept_connection(|| Err(std::io::Error::other("accept failed"))).unwrap_err();
    assert!(matches!(err, AuthError::Io(_)));
    let none =
        accept_connection(|| Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "later")))
            .unwrap();
    assert!(none.is_none());
}

#[test]
fn wait_for_callback_loops_until_timeout() {
    let (listener, _) = bind_loopback().unwrap();
    let err = wait_for_callback(&listener, "state", Duration::from_millis(80)).unwrap_err();
    assert!(err.to_string().contains("timed out"));
}

#[test]
fn callback_html_uses_the_dark_whycodes_theme() {
    let ok = callback_html(true);
    assert!(ok.contains("whycodes login complete"), "{ok}");
    assert!(
        ok.contains("You can close this tab and return to the terminal."),
        "{ok}"
    );
    assert!(ok.contains("color-scheme"), "{ok}");
    assert!(ok.contains("only dark"), "{ok}");
    assert!(ok.contains("#0a0a0a"), "{ok}");
    assert!(ok.contains("#fab283"), "{ok}");
    assert!(ok.contains("#7fd88f"), "{ok}");
    assert!(ok.contains("body class=\"ok\""), "{ok}");

    let fail = callback_html(false);
    assert!(fail.contains("whycodes login failed"), "{fail}");
    assert!(
        fail.contains("The provider did not return a code. You can close this tab."),
        "{fail}"
    );
    assert!(fail.contains("#0a0a0a"), "{fail}");
    assert!(fail.contains("#e06c75"), "{fail}");
    assert!(fail.contains("body class=\"err\""), "{fail}");
}

#[test]
fn origin_from_header_parses_origin_and_skips_other_lines() {
    assert_eq!(
        origin_from_header("Origin: https://accounts.x.ai\r\n").as_deref(),
        Some("https://accounts.x.ai")
    );
    assert_eq!(origin_from_header("Host: localhost\r\n"), None);
    assert_eq!(origin_from_header("NotAHeader\r\n"), None);
}

#[test]
fn authorize_url_includes_pkce_and_extra_params() {
    let pkce = crate::pkce::Pkce::new();
    let url = authorize_url(
        "https://example.com/oauth/authorize",
        "client-1",
        "http://127.0.0.1:9/callback",
        "openid profile",
        &pkce,
        &[("audience".into(), "api".into())],
    );
    assert!(
        url.starts_with("https://example.com/oauth/authorize?"),
        "{url}"
    );
    assert!(url.contains("response_type=code"), "{url}");
    assert!(url.contains("client_id=client-1"), "{url}");
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    assert!(url.contains("audience=api"), "{url}");
    assert!(url.contains(&format!("state={}", pkce.state)), "{url}");
    assert!(
        url.contains(&format!("code_challenge={}", pkce.challenge)),
        "{url}"
    );
}

#[test]
fn cors_headers_allow_known_origin_and_fallback() {
    let known = cors_headers(Some("https://accounts.x.ai"));
    assert!(
        known.contains("Access-Control-Allow-Origin: https://accounts.x.ai"),
        "{known}"
    );
    assert!(
        known.contains("Access-Control-Allow-Private-Network: true"),
        "{known}"
    );

    let unknown = cors_headers(Some("https://evil.example"));
    assert!(
        unknown.contains("Access-Control-Allow-Origin: https://accounts.x.ai"),
        "{unknown}"
    );

    let none = cors_headers(None);
    assert!(
        none.contains("Access-Control-Allow-Origin: https://accounts.x.ai"),
        "{none}"
    );
}
