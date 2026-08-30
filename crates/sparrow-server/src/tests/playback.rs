use std::{fmt::Write as _, time::Duration};

use axum::http::{Method, StatusCode, header};
use http_body_util::BodyExt as _;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};
use tower::ServiceExt as _;

use super::*;

const CHANNEL_NAME: &str = "Playback Fixture";
const TS_PACKET: [u8; 188] = {
    let mut packet = [0_u8; 188];
    packet[0] = 0x47;
    packet
};

#[tokio::test]
async fn playback_route_accepts_only_an_authenticated_channel_identifier() {
    let provider = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind no-contact provider");
    let location = format!(
        "http://provider-user:provider-secret@{}/live?token=provider-canary",
        provider.local_addr().expect("provider address")
    );
    let (app, channel_id) = playback_app(&location).await;

    for response in [
        send(
            &app.router,
            request(Method::GET, &format!("/api/v1/play/{channel_id}"), None),
        )
        .await,
        send(
            &app.router,
            request(
                Method::GET,
                &format!("/api/v1/play/{channel_id}"),
                Some("wrong-password"),
            ),
        )
        .await,
    ] {
        assert_authentication_required(&response);
    }

    let query_canary = "http%3A%2F%2Farbitrary.fixture.invalid%2Fprivate";
    let query = send(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/play/{channel_id}?url={query_canary}"),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_invalid_input(&query, "query", "invalid-format");
    assert!(!query.text.contains("arbitrary.fixture.invalid"));

    let malformed = send(
        &app.router,
        request(Method::GET, "/api/v1/play/not-a-channel-id", Some(PASSWORD)),
    )
    .await;
    assert_invalid_input(&malformed, "channel-id", "invalid-format");

    let missing_id = format!("ch1_{}", "0".repeat(64));
    let missing = send(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/play/{missing_id}"),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json,
        json!({ "error": { "_tag": "not-found", "resource": "channel" } })
    );
    assert!(!missing.text.contains(&missing_id));

    let old_proxy = send(
        &app.router,
        request(
            Method::GET,
            "/proxy/http%3A%2F%2Farbitrary.fixture.invalid%2Fprivate",
            Some(PASSWORD),
        ),
    )
    .await;
    assert_eq!(old_proxy.status, StatusCode::NOT_FOUND);
    let api_proxy = send(
        &app.router,
        request(
            Method::GET,
            "/api/v1/proxy/http%3A%2F%2Farbitrary.fixture.invalid%2Fprivate",
            Some(PASSWORD),
        ),
    )
    .await;
    assert_invalid_input(&api_proxy, "route", "invalid-format");

    assert!(
        timeout(Duration::from_millis(150), provider.accept())
            .await
            .is_err(),
        "rejected requests must not contact the resolved provider"
    );
}

#[tokio::test]
async fn successful_playback_relays_only_bytes_and_sparrow_owned_headers() {
    let provider_body = [&TS_PACKET[..], &TS_PACKET[..]].concat();
    let provider = TestProvider::one(ok_response(
        &[
            ("Content-Type", "application/provider-private"),
            ("Set-Cookie", "provider-session=private-cookie"),
            (
                "Location",
                "https://private-location.fixture.invalid/secret",
            ),
            ("Server", "private-provider-canary"),
            ("X-Provider-Canary", "private-header-value"),
            ("Access-Control-Allow-Origin", "*"),
        ],
        &provider_body,
    ))
    .await;
    let location = provider.credentialed_url(
        "upstream-user",
        "upstream-password",
        "/live/channel.ts?token=upstream-token-canary",
    );
    let (app, channel_id) = playback_app(&location).await;
    let mut request = request(
        Method::GET,
        &format!("/api/v1/play/{channel_id}"),
        Some(PASSWORD),
    );
    request
        .headers_mut()
        .insert(header::COOKIE, "browser-cookie=private".parse().unwrap());
    request
        .headers_mut()
        .insert(header::RANGE, "bytes=188-".parse().unwrap());
    request.headers_mut().insert(
        header::ORIGIN,
        "https://browser-origin.fixture.invalid".parse().unwrap(),
    );
    request.headers_mut().insert(
        header::REFERER,
        "https://browser-referer.fixture.invalid/private"
            .parse()
            .unwrap(),
    );
    request
        .headers_mut()
        .insert("x-client-canary", "private-client-header".parse().unwrap());

    let response = app
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("the infallible router responds");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp2t");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    for forbidden in [
        header::CONTENT_LENGTH,
        header::CONTENT_ENCODING,
        header::SET_COOKIE,
        header::LOCATION,
        header::SERVER,
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
    ] {
        assert!(!response.headers().contains_key(forbidden));
    }
    assert!(!response.headers().contains_key("x-provider-canary"));

    let body = response
        .into_body()
        .collect()
        .await
        .expect("playback bytes are readable")
        .to_bytes();
    assert_eq!(body.as_ref(), provider_body);

    let upstream = provider.request().await;
    assert!(upstream.starts_with("GET /live/channel.ts?token=upstream-token-canary HTTP/1.1\r\n"));
    assert!(
        upstream.contains("authorization: Basic dXBzdHJlYW0tdXNlcjp1cHN0cmVhbS1wYXNzd29yZA==\r\n")
    );
    assert!(upstream.contains("accept-encoding: identity\r\n"));
    assert!(upstream.contains("user-agent: sparrow-tv/0.0.0\r\n"));
    assert!(upstream.contains("accept: video/mp2t, application/octet-stream;q=0.9, */*;q=0.1\r\n"));
    for forbidden in [
        "browser-cookie",
        "bytes=188-",
        "browser-origin",
        "browser-referer",
        "private-client-header",
        PASSWORD,
    ] {
        assert!(!upstream.contains(forbidden));
    }
}

#[tokio::test]
async fn playback_header_failures_are_typed_actionable_and_privacy_safe() {
    for (status, http_status, reason, retryable) in [
        (401, StatusCode::FAILED_DEPENDENCY, "rejected", false),
        (408, StatusCode::GATEWAY_TIMEOUT, "timed-out", true),
        (429, StatusCode::SERVICE_UNAVAILABLE, "unavailable", true),
        (500, StatusCode::SERVICE_UNAVAILABLE, "unavailable", true),
        (204, StatusCode::BAD_GATEWAY, "invalid-response", false),
    ] {
        let provider = TestProvider::one(status_response(status, b"private-provider-body")).await;
        let location = provider.credentialed_url(
            "failure-user",
            "failure-password",
            "/failure?token=failure-location-canary",
        );
        let (app, channel_id) = playback_app(&location).await;
        let response = send(
            &app.router,
            request(
                Method::GET,
                &format!("/api/v1/play/{channel_id}"),
                Some(PASSWORD),
            ),
        )
        .await;

        assert_eq!(response.status, http_status, "provider status {status}");
        assert_eq!(
            response.json,
            json!({
                "error": {
                    "_tag": "playback-failed",
                    "reason": reason,
                    "retryable": retryable,
                }
            })
        );
        for canary in [
            "private-provider-body",
            "failure-user",
            "failure-password",
            "failure-location-canary",
            "http://",
        ] {
            assert!(!response.text.contains(canary));
        }
        let _ = provider.request().await;
    }

    let encoded = TestProvider::one(ok_response(
        &[("Content-Encoding", "gzip")],
        b"encoded-provider-body",
    ))
    .await;
    let location = encoded.url("/encoded");
    let (app, channel_id) = playback_app(&location).await;
    let response = send(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/play/{channel_id}"),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json,
        json!({
            "error": {
                "_tag": "playback-failed",
                "reason": "invalid-response",
                "retryable": false,
            }
        })
    );
    assert!(!response.text.contains("encoded-provider-body"));
    let _ = encoded.request().await;
}

#[tokio::test]
async fn dropping_downstream_body_promptly_cancels_upstream_connection() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind cancellable provider");
    let location = format!("http://{}/long-live", listener.local_addr().unwrap());
    let (request_tx, request_rx) = oneshot::channel();
    let (closed_tx, closed_rx) = oneshot::channel();
    let provider_task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.expect("accept playback request");
        let request = read_request(&mut connection).await;
        connection
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: 1048576\r\nConnection: keep-alive\r\n\r\n",
            )
            .await
            .expect("write playback headers");
        connection
            .write_all(&TS_PACKET)
            .await
            .expect("write first TS packet");
        connection.flush().await.expect("flush first TS packet");
        let _ = request_tx.send(request);

        let mut byte = [0_u8; 1];
        let closed = connection
            .read(&mut byte)
            .await
            .expect("observe client close")
            == 0;
        let _ = closed_tx.send(closed);
    });
    let (app, channel_id) = playback_app(&location).await;
    let response = app
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/play/{channel_id}"),
            Some(PASSWORD),
        ))
        .await
        .expect("the infallible router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let frame = timeout(Duration::from_secs(1), body.frame())
        .await
        .expect("first playback frame arrives")
        .expect("playback body has a frame")
        .expect("first playback frame is readable");
    assert_eq!(
        frame.into_data().expect("frame contains data").as_ref(),
        TS_PACKET.as_slice()
    );
    let upstream_request = request_rx.await.expect("provider captured request");
    assert!(upstream_request.starts_with("GET /long-live HTTP/1.1\r\n"));

    drop(body);
    assert!(
        timeout(Duration::from_millis(500), closed_rx)
            .await
            .expect("upstream closes promptly")
            .expect("provider reports close"),
        "dropping the downstream body must close the provider connection"
    );
    provider_task.await.expect("provider task exits");
}

#[tokio::test]
async fn real_downstream_disconnect_before_headers_cancels_the_pending_upstream_request() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind pending provider");
    let location = format!("http://{}/pending", listener.local_addr().unwrap());
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let (closed_tx, closed_rx) = oneshot::channel();
    let provider_task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.expect("accept pending request");
        let request = read_request(&mut connection).await;
        let _ = accepted_tx.send(request);
        let mut byte = [0_u8; 1];
        let closed = connection
            .read(&mut byte)
            .await
            .expect("observe client close")
            == 0;
        let _ = closed_tx.send(closed);
    });
    let (app, channel_id) = playback_app(&location).await;
    let server_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind real Axum listener");
    let server_address = server_listener.local_addr().expect("Axum listener address");
    let server_router = app.router.clone();
    let server_task = tokio::spawn(async move {
        axum::serve(server_listener, server_router)
            .await
            .expect("fixture Axum server runs")
    });
    let mut downstream = TcpStream::connect(server_address)
        .await
        .expect("connect real downstream client");
    let request = format!(
        "GET /api/v1/play/{channel_id} HTTP/1.1\r\nHost: {server_address}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",
        basic(PASSWORD, "Basic")
    );
    downstream
        .write_all(request.as_bytes())
        .await
        .expect("write real downstream request");
    let upstream_request = timeout(Duration::from_secs(1), accepted_rx)
        .await
        .expect("provider receives pending request")
        .expect("provider reports pending request");
    assert!(upstream_request.starts_with("GET /pending HTTP/1.1\r\n"));

    drop(downstream);
    assert!(
        timeout(Duration::from_secs(1), closed_rx)
            .await
            .expect("pending upstream closes promptly")
            .expect("provider reports close"),
        "dropping the pending handler must close the provider connection"
    );
    provider_task.await.expect("pending provider task exits");
    server_task.abort();
    assert!(
        server_task
            .await
            .expect_err("fixture Axum server is stopped")
            .is_cancelled()
    );
}

#[tokio::test]
async fn body_stage_errors_remain_privacy_safe_after_headers_are_committed() {
    let provider = TestProvider::one(truncated_response(&TS_PACKET, 1024)).await;
    let location = provider.credentialed_url(
        "body-user",
        "body-password",
        "/truncated?token=body-location-canary",
    );
    let (app, channel_id) = playback_app(&location).await;
    let response = app
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/play/{channel_id}"),
            Some(PASSWORD),
        ))
        .await
        .expect("the infallible router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let error = response
        .into_body()
        .collect()
        .await
        .expect_err("truncated provider body interrupts playback");
    let diagnostics = format!("{error:?} {error}");
    for canary in [
        "body-user",
        "body-password",
        "body-location-canary",
        "truncated",
        "http://",
    ] {
        assert!(!diagnostics.contains(canary));
    }
    let _ = provider.request().await;
}

async fn playback_app(location: &str) -> (TestApp, String) {
    let m3u = format!(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"playback.fixture\" group-title=\"Live\",{CHANNEL_NAME}\n{location}\n"
    );
    let app = TestApp::fixture(m3u.as_bytes()).await;
    let channels = get_json(&app.router, "/api/v1/channels?limit=10").await;
    let id = channel_id_named(&channels, CHANNEL_NAME).to_owned();
    (app, id)
}

fn ok_response(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        writeln!(response, "{name}: {value}\r").expect("write provider header");
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

fn truncated_response(body: &[u8], declared_length: usize) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

struct TestProvider {
    authority: String,
    requests: oneshot::Receiver<Vec<String>>,
}

impl TestProvider {
    async fn one(response: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider fixture");
        let authority = listener.local_addr().expect("provider address").to_string();
        let (requests_tx, requests) = oneshot::channel();
        tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.expect("accept provider request");
            let request = read_request(&mut connection).await;
            connection
                .write_all(&response)
                .await
                .expect("write provider response");
            let _ = connection.shutdown().await;
            let _ = requests_tx.send(vec![request]);
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
        let mut requests = self.requests.await.expect("captured provider requests");
        assert_eq!(requests.len(), 1);
        requests.pop().expect("one captured provider request")
    }
}

async fn read_request(connection: &mut tokio::net::TcpStream) -> String {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = connection
            .read(&mut chunk)
            .await
            .expect("read provider request");
        if count == 0 {
            break;
        }
        received.extend_from_slice(&chunk[..count]);
        if received.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        assert!(received.len() <= 16 * 1024, "request headers are bounded");
    }
    String::from_utf8(received).expect("provider request is ASCII")
}
