use std::{
    fmt::Write as _,
    io::Write as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use flate2::{Compression, write::GzEncoder};
use futures_util::TryStreamExt;
use reqwest::header::AUTHORIZATION;
use static_assertions::assert_not_impl_any;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    time::{sleep, timeout},
};

use super::*;

assert_not_impl_any!(HttpFetch: Debug);

#[tokio::test]
async fn sends_private_conditionals_and_returns_not_modified_validators() {
    let response = concat!(
        "HTTP/1.1 304 Not Modified\r\n",
        "ETag: \"second\"\r\n",
        "Last-Modified: Sun, 06 Nov 1994 08:49:37 GMT\r\n",
        "Connection: close\r\n",
        "\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = TestServer::one(response).await;
    let retained = PrivateSourceValidators::parse(
        Some("\"first\"".to_owned()),
        Some("Sun, 06 Nov 1994 08:00:00 GMT".to_owned()),
    )
    .expect("valid retained validators");
    let adapter = test_adapter();
    let built_request = apply_conditionals(adapter.client.get(server.url("/guide.xml")), &retained)
        .expect("conditional headers")
        .build()
        .expect("conditional request");
    assert!(built_request.headers()[IF_NONE_MATCH].is_sensitive());
    assert!(built_request.headers()[IF_MODIFIED_SINCE].is_sensitive());

    let fetch = adapter
        .fetch(&server.url("/guide.xml"), &retained)
        .await
        .expect("304 response");
    let HttpFetch::NotModified { validators } = fetch else {
        panic!("expected a not-modified response");
    };
    assert_eq!(validators.expose_etag(), Some("\"second\""));
    assert_eq!(
        validators.expose_last_modified(),
        Some("Sun, 06 Nov 1994 08:49:37 GMT")
    );

    let request = server.request().await;
    assert!(request.contains("if-none-match: \"first\"\r\n"));
    assert!(request.contains("if-modified-since: Sun, 06 Nov 1994 08:00:00 GMT\r\n"));
}

#[tokio::test]
async fn streams_decompressed_bytes_without_claiming_the_encoded_length() {
    let decoded = b"#EXTM3U\n#EXTINF:-1,One\nhttps://media.invalid/one\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(decoded).expect("compress fixture");
    let encoded = encoder.finish().expect("finish fixture");
    let mut response = format!(
        concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Encoding: gzip\r\n",
            "Content-Length: {}\r\n",
            "ETag: \"compressed\"\r\n",
            "Connection: close\r\n",
            "\r\n"
        ),
        encoded.len()
    )
    .into_bytes();
    response.extend_from_slice(&encoded);
    let server = TestServer::one(response).await;

    let fetch = test_adapter()
        .fetch(
            &server.url("/channels.m3u"),
            &PrivateSourceValidators::default(),
        )
        .await
        .expect("200 response");
    let HttpFetch::Modified {
        declared_decoded_length,
        decoded_body,
        validators,
    } = fetch
    else {
        panic!("expected a modified response");
    };
    let body = collect_body(decoded_body)
        .await
        .expect("stream decompressed body");

    assert_eq!(declared_decoded_length, None);
    assert_eq!(&body[..], decoded);
    assert_eq!(validators.expose_etag(), Some("\"compressed\""));
}

#[tokio::test]
async fn credentialed_url_becomes_sensitive_basic_authorization() {
    let server = TestServer::one(ok_response(b"#EXTM3U\n")).await;
    let location = server.credentialed_url("private-user", "private-password", "/source?token=x");
    let adapter = test_adapter();
    let built_request = adapter
        .client
        .get(&location)
        .build()
        .expect("build credentialed request");

    assert_eq!(built_request.url().username(), "");
    assert_eq!(built_request.url().password(), None);
    assert!(built_request.headers()[AUTHORIZATION].is_sensitive());

    let _ = adapter
        .fetch(&location, &PrivateSourceValidators::default())
        .await
        .expect("credentialed request");
    let request = server.request().await;

    assert!(request.starts_with("GET /source?token=x HTTP/1.1\r\n"));
    assert!(request.contains("authorization: Basic cHJpdmF0ZS11c2VyOnByaXZhdGUtcGFzc3dvcmQ=\r\n"));
    let diagnostics = format!("{adapter:?}");
    assert!(!diagnostics.contains("private-user"));
    assert!(!diagnostics.contains("private-password"));
    assert_eq!(diagnostics, "HttpSourceAccess { .. }");
}

#[tokio::test]
async fn follows_same_origin_redirect_without_dropping_private_request_headers() {
    let server =
        TestServer::sequence(vec![redirect_response("/final"), ok_response(b"ready")]).await;
    let validators =
        PrivateSourceValidators::parse(Some("\"redirect-validator\"".to_owned()), None)
            .expect("valid validator");
    let adapter = test_adapter();

    let fetch = adapter
        .fetch(
            &server.credentialed_url("redirect-user", "redirect-password", "/start"),
            &validators,
        )
        .await
        .expect("same-origin redirect");
    let HttpFetch::Modified { decoded_body, .. } = fetch else {
        panic!("expected a modified response");
    };
    assert_eq!(collect_body(decoded_body).await, Ok(b"ready".to_vec()));

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /start HTTP/1.1\r\n"));
    assert!(requests[1].starts_with("GET /final HTTP/1.1\r\n"));
    for request in requests {
        assert!(
            request
                .contains("authorization: Basic cmVkaXJlY3QtdXNlcjpyZWRpcmVjdC1wYXNzd29yZA==\r\n")
        );
        assert!(request.contains("if-none-match: \"redirect-validator\"\r\n"));
    }
}

#[tokio::test]
async fn rejects_cross_origin_redirect_without_forwarding_credentials() {
    let target = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind redirect target");
    let target_location = format!(
        "http://{}/captured",
        target.local_addr().expect("redirect target address")
    );
    let source = TestServer::one(redirect_response(&target_location)).await;
    let adapter = test_adapter();

    let failure = expect_failure(
        adapter
            .fetch(
                &source.credentialed_url("cross-user", "cross-password", "/start"),
                &PrivateSourceValidators::parse(Some("\"cross-validator\"".to_owned()), None)
                    .expect("valid validator"),
            )
            .await,
    );

    assert_eq!(failure.reason, SourceAccessError::InvalidResponse);
    assert!(
        timeout(Duration::from_millis(100), target.accept())
            .await
            .is_err()
    );
    let source_request = source.request().await;
    assert!(
        source_request.contains("authorization: Basic Y3Jvc3MtdXNlcjpjcm9zcy1wYXNzd29yZA==\r\n")
    );
}

#[tokio::test]
async fn bounds_same_origin_redirect_chains() {
    let server = RedirectLoopServer::start().await;

    let failure = expect_failure(
        test_adapter()
            .fetch(&server.url("/loop"), &PrivateSourceValidators::default())
            .await,
    );

    assert_eq!(failure.reason, SourceAccessError::InvalidResponse);
    assert_eq!(server.request_count(), MAX_REDIRECTS + 1);
}

#[tokio::test]
async fn maps_statuses_without_retaining_private_locations() {
    let cases = [
        (401, SourceAccessError::Rejected),
        (404, SourceAccessError::Rejected),
        (408, SourceAccessError::TimedOut),
        (500, SourceAccessError::Unavailable),
        (204, SourceAccessError::InvalidResponse),
        (302, SourceAccessError::InvalidResponse),
    ];

    for (status, expected) in cases {
        let server = TestServer::one(empty_response(status, &[])).await;
        let private_location =
            server.credentialed_url("canary-user", "canary-password", "/private");
        let failure = expect_failure(
            test_adapter()
                .fetch(&private_location, &PrivateSourceValidators::default())
                .await,
        );
        assert_eq!(failure.reason, expected);
        let diagnostics = format!("{failure:?}");
        assert!(!diagnostics.contains("canary-user"));
        assert!(!diagnostics.contains("canary-password"));
        assert!(!diagnostics.contains("private"));
    }
}

#[tokio::test]
async fn accepts_delta_and_http_date_retry_after_only_on_retryable_statuses() {
    let delta_server = TestServer::one(empty_response(429, &[("Retry-After", "300")])).await;
    let delta = expect_failure(
        test_adapter()
            .fetch(
                &delta_server.url("/limited"),
                &PrivateSourceValidators::default(),
            )
            .await,
    );
    assert_eq!(delta.reason, SourceAccessError::Unavailable);
    assert_eq!(delta.retry_after, Some(Duration::from_secs(300)));
    let source_failure = delta.into_source_failure();
    assert_eq!(source_failure.reason(), SourceAccessError::Unavailable);
    assert_eq!(source_failure.retry_after(), Some(Duration::from_secs(300)));

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_717);
    assert_eq!(
        parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now),
        Some(Duration::from_secs(60))
    );
    assert_eq!(parse_retry_after("not-a-delay", now), None);
    assert_eq!(parse_retry_after("+30", now), None);

    let invalid_server =
        TestServer::one(empty_response(503, &[("Retry-After", "not-a-delay")])).await;
    let invalid = expect_failure(
        test_adapter()
            .fetch(
                &invalid_server.url("/busy"),
                &PrivateSourceValidators::default(),
            )
            .await,
    );
    assert_eq!(invalid.reason, SourceAccessError::Unavailable);
    assert_eq!(invalid.retry_after, None);

    let rejected_server = TestServer::one(empty_response(401, &[("Retry-After", "600")])).await;
    let rejected = expect_failure(
        test_adapter()
            .fetch(
                &rejected_server.url("/unauthorized"),
                &PrivateSourceValidators::default(),
            )
            .await,
    );
    assert_eq!(rejected.reason, SourceAccessError::Rejected);
    assert_eq!(rejected.retry_after, None);

    for status in [408, 500, 504] {
        let server = TestServer::one(empty_response(status, &[("Retry-After", "90")])).await;
        let failure = expect_failure(
            test_adapter()
                .fetch(
                    &server.url("/retryable"),
                    &PrivateSourceValidators::default(),
                )
                .await,
        );
        assert_eq!(failure.retry_after, Some(Duration::from_secs(90)));
    }
}

#[tokio::test]
async fn rejects_not_modified_without_a_conditional_request() {
    let server =
        TestServer::one(b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_vec()).await;

    let failure = expect_failure(
        test_adapter()
            .fetch(
                &server.url("/first-load"),
                &PrivateSourceValidators::default(),
            )
            .await,
    );

    assert_eq!(failure.reason, SourceAccessError::InvalidResponse);
}

#[tokio::test]
async fn maps_truncated_body_to_interrupted_read() {
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(b"short");
    let server = TestServer::one(response).await;

    let fetch = test_adapter()
        .fetch(
            &server.url("/truncated"),
            &PrivateSourceValidators::default(),
        )
        .await
        .expect("response headers");
    let HttpFetch::Modified { decoded_body, .. } = fetch else {
        panic!("expected a modified response");
    };

    assert_eq!(
        collect_body(decoded_body).await,
        Err(SourceReadError::Interrupted)
    );
}

#[tokio::test]
async fn maps_malformed_compressed_body_to_invalid_read() {
    let encoded = b"not-gzip";
    let mut response = format!(
        concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Encoding: gzip\r\n",
            "Content-Length: {}\r\n",
            "Connection: close\r\n",
            "\r\n"
        ),
        encoded.len()
    )
    .into_bytes();
    response.extend_from_slice(encoded);
    let server = TestServer::one(response).await;

    let fetch = test_adapter()
        .fetch(
            &server.url("/malformed-gzip"),
            &PrivateSourceValidators::default(),
        )
        .await
        .expect("response headers");
    let HttpFetch::Modified { decoded_body, .. } = fetch else {
        panic!("expected a modified response");
    };

    assert_eq!(
        collect_body(decoded_body).await,
        Err(SourceReadError::InvalidBody)
    );
}

#[tokio::test]
async fn resetting_read_timeout_interrupts_a_stalled_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stalled server");
    let location = format!(
        "http://{}/stalled",
        listener.local_addr().expect("stalled server address")
    );
    let _server_task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.expect("accept stalled request");
        let _ = read_request(&mut connection).await;
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nx")
            .await
            .expect("write first body byte");
        sleep(Duration::from_millis(200)).await;
        let _ = connection.write_all(b"y").await;
    });
    let adapter = HttpSourceAccess {
        client: production_client_builder()
            .read_timeout(Duration::from_millis(30))
            .no_proxy()
            .build()
            .expect("build short-timeout client"),
    };

    let fetch = adapter
        .fetch(&location, &PrivateSourceValidators::default())
        .await
        .expect("response headers");
    let HttpFetch::Modified { decoded_body, .. } = fetch else {
        panic!("expected a modified response");
    };

    assert_eq!(
        collect_body(decoded_body).await,
        Err(SourceReadError::Interrupted)
    );
}

#[tokio::test]
async fn returns_from_non_success_headers_without_consuming_the_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind error-body server");
    let location = format!(
        "http://{}/error-body",
        listener.local_addr().expect("error-body server address")
    );
    let (release_body, released_body) = oneshot::channel();
    let _server_task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.expect("accept error request");
        let _ = read_request(&mut connection).await;
        connection
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 12\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("write error headers");
        let _ = released_body.await;
        let _ = connection.write_all(b"private-body").await;
    });

    let result = timeout(
        Duration::from_millis(100),
        test_adapter().fetch(&location, &PrivateSourceValidators::default()),
    )
    .await
    .expect("adapter returns from error headers");
    let failure = expect_failure(result);
    assert_eq!(failure.reason, SourceAccessError::Unavailable);
    let _ = release_body.send(());
}

fn expect_failure(result: Result<HttpFetch, HttpFailure>) -> HttpFailure {
    match result {
        Ok(_) => panic!("expected source access to fail"),
        Err(failure) => failure,
    }
}

async fn collect_body(body: SourceByteStream) -> Result<Vec<u8>, SourceReadError> {
    body.try_fold(Vec::new(), |mut collected, chunk| async move {
        collected.extend_from_slice(&chunk);
        Ok(collected)
    })
    .await
}

fn test_adapter() -> HttpSourceAccess {
    HttpSourceAccess {
        client: production_client_builder()
            .no_proxy()
            .build()
            .expect("build test client"),
    }
}

fn ok_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn empty_response(status: u16, headers: &[(&str, &str)]) -> Vec<u8> {
    let reason = match status {
        204 => "No Content",
        302 => "Found",
        401 => "Unauthorized",
        404 => "Not Found",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Test",
    };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        writeln!(response, "{name}: {value}\r").expect("write response header");
    }
    response.push_str("Content-Length: 0\r\nConnection: close\r\n\r\n");
    response.into_bytes()
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
            .expect("bind test server");
        let authority = listener.local_addr().expect("server address").to_string();
        let (requests_tx, requests) = oneshot::channel();

        let _server_task = tokio::spawn(async move {
            let mut received = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut connection, _) = listener.accept().await.expect("accept request");
                received.push(read_request(&mut connection).await);
                connection
                    .write_all(&response)
                    .await
                    .expect("write response");
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
        let mut requests = self.requests().await;
        assert_eq!(requests.len(), 1);
        requests.pop().expect("one captured request")
    }

    async fn requests(self) -> Vec<String> {
        self.requests.await.expect("captured requests")
    }
}

async fn read_request(connection: &mut tokio::net::TcpStream) -> String {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = connection.read(&mut chunk).await.expect("read request");
        if count == 0 {
            break;
        }
        received.extend_from_slice(&chunk[..count]);
        if received.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        assert!(received.len() <= 16 * 1024, "request headers are bounded");
    }
    String::from_utf8(received).expect("ASCII request")
}

struct RedirectLoopServer {
    authority: String,
    requests: Arc<AtomicUsize>,
}

impl RedirectLoopServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect loop server");
        let authority = listener
            .local_addr()
            .expect("redirect loop address")
            .to_string();
        let requests = Arc::new(AtomicUsize::new(0));
        let task_requests = Arc::clone(&requests);
        let _server_task = tokio::spawn(async move {
            loop {
                let Ok((mut connection, _)) = listener.accept().await else {
                    return;
                };
                task_requests.fetch_add(1, Ordering::SeqCst);
                let _ = read_request(&mut connection).await;
                let _ = connection.write_all(&redirect_response("/loop")).await;
                let _ = connection.shutdown().await;
            }
        });
        Self {
            authority,
            requests,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.authority)
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}
