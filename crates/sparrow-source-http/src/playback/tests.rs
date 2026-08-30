use std::{fmt::Write as _, time::Duration};

use reqwest::{
    Url,
    header::{
        AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, PROXY_AUTHORIZATION,
        WWW_AUTHENTICATE,
    },
};
use static_assertions::assert_not_impl_any;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::oneshot,
    time::timeout,
};

use super::*;

assert_not_impl_any!(PlaybackResponse: std::fmt::Debug, std::fmt::Display);

#[tokio::test]
async fn sends_only_fixed_headers_and_preserves_same_origin_url_authentication() {
    let server = TestServer::sequence(vec![
        redirect_response("/final"),
        ok_response(&[("X-Private", "response-canary")], b"mpeg-ts-bytes"),
    ])
    .await;
    let adapter = test_adapter();
    let location = Url::parse(&server.credentialed_url(
        "provider-user",
        "provider-password",
        "/start?token=location-canary",
    ))
    .expect("fixture playback location is valid");

    let response = adapter
        .fetch(&location)
        .await
        .expect("same-origin playback opens");
    let body = response
        .into_body()
        .try_fold(Vec::new(), |mut bytes, chunk| async move {
            bytes.extend_from_slice(&chunk);
            Ok(bytes)
        })
        .await
        .expect("fixture playback body is readable");
    assert_eq!(body, b"mpeg-ts-bytes");

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /start?token=location-canary HTTP/1.1\r\n"));
    assert!(requests[1].starts_with("GET /final HTTP/1.1\r\n"));
    for request in requests {
        assert!(
            request
                .contains("authorization: Basic cHJvdmlkZXItdXNlcjpwcm92aWRlci1wYXNzd29yZA==\r\n")
        );
        assert!(request.contains("accept-encoding: identity\r\n"));
        assert!(request.contains("user-agent: sparrow-tv/0.0.0\r\n"));
        assert!(
            request.contains("accept: video/mp2t, application/octet-stream;q=0.9, */*;q=0.1\r\n")
        );
        assert!(!request.contains("referer:"));
        assert!(!request.contains("cookie:"));
        assert!(!request.contains("range:"));
    }

    let diagnostics = format!("{adapter:?}");
    assert_eq!(diagnostics, "HttpPlaybackAccess { .. }");
    for canary in [
        "provider-user",
        "provider-password",
        "location-canary",
        "response-canary",
    ] {
        assert!(!diagnostics.contains(canary));
    }
}

#[tokio::test]
async fn follows_cross_origin_cdn_redirect_without_forwarding_sensitive_headers() {
    let target = TestServer::one(ok_response(&[], b"cdn-mpeg-ts-bytes")).await;
    let source = TestServer::one(redirect_response(&target.url("/cdn?signature=cdn-canary"))).await;
    let location = Url::parse(&source.credentialed_url(
        "redirect-user",
        "redirect-password",
        "/start?token=source-canary",
    ))
    .expect("fixture playback location is valid");
    let adapter = test_adapter_with_sensitive_defaults();

    let response = adapter
        .fetch(&location)
        .await
        .expect("cross-origin CDN playback opens");
    assert_eq!(read_body(response).await, b"cdn-mpeg-ts-bytes");

    let source_request = source.request().await;
    assert!(
        source_request
            .contains("authorization: Basic cmVkaXJlY3QtdXNlcjpyZWRpcmVjdC1wYXNzd29yZA==\r\n")
    );
    assert!(source_request.contains("cookie: session=source-cookie-canary\r\n"));
    assert!(source_request.contains("cookie2: legacy-source-cookie-canary\r\n"));
    assert!(source_request.contains("proxy-authorization: Basic proxy-canary\r\n"));
    assert!(source_request.contains("www-authenticate: source-challenge-canary\r\n"));

    let target_request = target.request().await;
    assert!(target_request.starts_with("GET /cdn?signature=cdn-canary HTTP/1.1\r\n"));
    for sensitive_header in [
        "authorization:",
        "cookie:",
        "cookie2:",
        "proxy-authorization:",
        "www-authenticate:",
    ] {
        assert!(
            !target_request.contains(sensitive_header),
            "redirected request leaked {sensitive_header}"
        );
    }
    assert!(target_request.contains("accept-encoding: identity\r\n"));
    assert!(target_request.contains("user-agent: sparrow-tv/0.0.0\r\n"));
    assert!(
        target_request
            .contains("accept: video/mp2t, application/octet-stream;q=0.9, */*;q=0.1\r\n")
    );
    assert!(!target_request.contains("referer:"));
}

#[tokio::test]
async fn rejects_credentialed_redirect_target_before_contact() {
    let target = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind unsafe redirect target");
    let target_location = format!(
        "http://echoed-user:echoed-password@{}/captured",
        target.local_addr().expect("unsafe target address")
    );
    let source = TestServer::one(redirect_response(&target_location)).await;
    let location = Url::parse(&source.url("/start?token=source-canary"))
        .expect("fixture playback location is valid");

    let failure = expect_failure(test_adapter().fetch(&location).await);

    assert_eq!(failure, PlaybackAccessError::InvalidResponse);
    assert!(
        timeout(Duration::from_millis(100), target.accept())
            .await
            .is_err()
    );
    let _ = source.request().await;
    let diagnostics = format!("{failure:?} {failure}");
    for canary in [
        "echoed-user",
        "echoed-password",
        "source-canary",
        &target_location,
    ] {
        assert!(!diagnostics.contains(canary));
    }
}

#[tokio::test]
async fn rejects_redirect_loops_and_excessive_chains() {
    let loop_server = TestServer::one(redirect_response("/loop")).await;
    let loop_location =
        Url::parse(&loop_server.url("/loop")).expect("fixture loop location is valid");

    let loop_failure = expect_failure(test_adapter().fetch(&loop_location).await);

    assert_eq!(loop_failure, PlaybackAccessError::InvalidResponse);
    let _ = loop_server.request().await;

    let responses = (0..=MAX_PLAYBACK_REDIRECTS)
        .map(|hop| redirect_response(&format!("/hop/{hop}")))
        .collect();
    let chain_server = TestServer::sequence(responses).await;
    let chain_location =
        Url::parse(&chain_server.url("/start")).expect("fixture redirect chain is valid");

    let chain_failure = expect_failure(test_adapter().fetch(&chain_location).await);

    assert_eq!(chain_failure, PlaybackAccessError::InvalidResponse);
    assert_eq!(
        chain_server.requests().await.len(),
        MAX_PLAYBACK_REDIRECTS + 1
    );
}

#[tokio::test]
async fn malformed_redirect_is_a_private_invalid_response() {
    let server = TestServer::one(redirect_response("http://[malformed-target")).await;
    let location = Url::parse(&server.url("/start?token=source-canary"))
        .expect("fixture playback location is valid");

    let failure = expect_failure(test_adapter().fetch(&location).await);

    assert_eq!(failure, PlaybackAccessError::InvalidResponse);
    let _ = server.request().await;
    let diagnostics = format!("{failure:?} {failure}");
    assert!(!diagnostics.contains("malformed-target"));
    assert!(!diagnostics.contains("source-canary"));
}

#[test]
fn rejects_non_http_downgrade_credentialed_and_loop_redirects() {
    let http = Url::parse("http://source.test/start").expect("valid HTTP URL");
    let https = Url::parse("https://source.test/start").expect("valid HTTPS URL");

    assert!(!playback_redirect_is_safe(
        std::slice::from_ref(&http),
        &Url::parse("ftp://cdn.test/live").expect("valid non-HTTP URL")
    ));
    assert!(!playback_redirect_is_safe(
        std::slice::from_ref(&https),
        &Url::parse("http://cdn.test/live").expect("valid downgrade URL")
    ));
    assert!(!playback_redirect_is_safe(
        std::slice::from_ref(&http),
        &Url::parse("http://user:password@cdn.test/live").expect("valid credentialed URL")
    ));
    assert!(!playback_redirect_is_safe(
        std::slice::from_ref(&http),
        &http
    ));
}

#[tokio::test]
async fn classifies_provider_status_without_reading_or_retaining_error_bodies() {
    for (status, expected) in [
        (401, PlaybackAccessError::Rejected),
        (404, PlaybackAccessError::Rejected),
        (408, PlaybackAccessError::TimedOut),
        (429, PlaybackAccessError::Unavailable),
        (500, PlaybackAccessError::Unavailable),
        (204, PlaybackAccessError::InvalidResponse),
    ] {
        let server = TestServer::one(status_response(status, b"private-error-body")).await;
        let location = Url::parse(&server.url("/private-error?token=error-canary"))
            .expect("fixture playback location is valid");
        let failure = expect_failure(test_adapter().fetch(&location).await);

        assert_eq!(failure, expected, "status {status}");
        assert_eq!(failure.retryable(), matches!(status, 408 | 429 | 500));
        let diagnostics = format!("{failure:?} {failure}");
        assert!(!diagnostics.contains("private-error-body"));
        assert!(!diagnostics.contains("error-canary"));
        let _ = server.request().await;
    }
}

#[tokio::test]
async fn returns_from_error_headers_without_waiting_for_the_private_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind gated error provider");
    let location = Url::parse(&format!(
        "http://{}/gated-error?token=private-location-canary",
        listener.local_addr().expect("gated provider address")
    ))
    .expect("fixture playback location is valid");
    let (release_tx, release_rx) = oneshot::channel();
    let provider = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.expect("accept gated request");
        let _ = read_request(&mut connection).await;
        connection
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 21\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("write gated error headers");
        let _ = release_rx.await;
        let _ = connection.write_all(b"private-provider-body").await;
    });

    let failure = timeout(Duration::from_millis(200), test_adapter().fetch(&location))
        .await
        .expect("adapter returns from provider headers");
    assert_eq!(expect_failure(failure), PlaybackAccessError::Unavailable);
    let _ = release_tx.send(());
    provider.await.expect("gated provider exits");
}

#[tokio::test]
async fn rejects_encoded_playback_instead_of_misrepresenting_provider_bytes() {
    for encodings in [
        vec![("Content-Encoding", "gzip")],
        vec![("Content-Encoding", "identity, gzip")],
        vec![
            ("Content-Encoding", "identity"),
            ("Content-Encoding", "gzip"),
        ],
        vec![
            ("Content-Encoding", "identity"),
            ("Content-Encoding", "identity"),
        ],
    ] {
        let server = TestServer::one(ok_response(&encodings, b"not-raw-mpeg-ts")).await;
        let location =
            Url::parse(&server.url("/encoded")).expect("fixture playback location is valid");

        let failure = expect_failure(test_adapter().fetch(&location).await);

        assert_eq!(failure, PlaybackAccessError::InvalidResponse);
        let request = server.request().await;
        assert!(request.contains("accept-encoding: identity\r\n"));
    }
}

fn test_adapter() -> HttpPlaybackAccess {
    HttpPlaybackAccess {
        client: playback_client_builder()
            .no_proxy()
            .build()
            .expect("build loopback playback client"),
    }
}

fn test_adapter_with_sensitive_defaults() -> HttpPlaybackAccess {
    let mut sensitive_headers = HeaderMap::new();
    sensitive_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer default-origin-canary"),
    );
    sensitive_headers.insert(
        COOKIE,
        HeaderValue::from_static("session=source-cookie-canary"),
    );
    sensitive_headers.insert(
        HeaderName::from_static("cookie2"),
        HeaderValue::from_static("legacy-source-cookie-canary"),
    );
    sensitive_headers.insert(
        PROXY_AUTHORIZATION,
        HeaderValue::from_static("Basic proxy-canary"),
    );
    sensitive_headers.insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("source-challenge-canary"),
    );

    HttpPlaybackAccess {
        client: playback_client_builder()
            .default_headers(sensitive_headers)
            .no_proxy()
            .build()
            .expect("build playback client with sensitive origin defaults"),
    }
}

async fn read_body(response: PlaybackResponse) -> Vec<u8> {
    response
        .into_body()
        .try_fold(Vec::new(), |mut bytes, chunk| async move {
            bytes.extend_from_slice(&chunk);
            Ok(bytes)
        })
        .await
        .expect("fixture playback body is readable")
}

fn expect_failure(result: Result<PlaybackResponse, PlaybackAccessError>) -> PlaybackAccessError {
    match result {
        Ok(_) => panic!("expected playback access to fail"),
        Err(failure) => failure,
    }
}

fn ok_response(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        writeln!(response, "{name}: {value}\r").expect("write fixture response header");
    }
    response.push_str("Connection: close\r\n\r\n");
    let mut response = response.into_bytes();
    response.extend_from_slice(body);
    response
}

fn status_response(status: u16, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Test",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn redirect_response(location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

struct TestServer {
    authority: String,
    requests: oneshot::Receiver<Vec<String>>,
}

impl TestServer {
    async fn one(response: Vec<u8>) -> Self {
        Self::sequence(vec![response]).await
    }

    async fn sequence(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind playback fixture server");
        let authority = listener.local_addr().expect("fixture address").to_string();
        let (requests_tx, requests) = oneshot::channel();
        tokio::spawn(async move {
            let mut received = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut connection, _) = listener.accept().await.expect("accept playback request");
                received.push(read_request(&mut connection).await);
                connection
                    .write_all(&response)
                    .await
                    .expect("write playback response");
                let _ = connection.shutdown().await;
            }
            let _ = requests_tx.send(received);
        });
        Self {
            authority,
            requests,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.authority)
    }

    fn credentialed_url(&self, username: &str, password: &str, path: &str) -> String {
        format!("http://{username}:{password}@{}{path}", self.authority)
    }

    async fn request(self) -> String {
        let mut requests = self.requests.await.expect("captured playback requests");
        assert_eq!(requests.len(), 1);
        requests.pop().expect("one captured playback request")
    }

    async fn requests(self) -> Vec<String> {
        self.requests.await.expect("captured playback requests")
    }
}

async fn read_request(connection: &mut tokio::net::TcpStream) -> String {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = connection
            .read(&mut chunk)
            .await
            .expect("read playback request");
        if count == 0 {
            break;
        }
        received.extend_from_slice(&chunk[..count]);
        if received.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        assert!(received.len() <= 16 * 1024, "request headers are bounded");
    }
    String::from_utf8(received).expect("playback request is ASCII")
}
