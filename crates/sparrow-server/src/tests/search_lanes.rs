use super::*;

#[tokio::test]
async fn lane_search_endpoints_project_independent_pages_and_compatible_cursors() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;

    let combined = get_json(
        &app.router,
        "/api/v1/search?term=fallback&channelLimit=1&programmeLimit=1",
    )
    .await;
    let channels = get_json(&app.router, "/api/v1/search/channels?term=fallback&limit=1").await;
    let programmes = get_json(
        &app.router,
        "/api/v1/search/programmes?term=fallback&limit=1",
    )
    .await;

    assert_eq!(channels, combined["channels"]);
    assert_eq!(programmes, combined["programmes"]);
    assert_eq!(channels["items"].as_array().unwrap().len(), 1);
    assert_eq!(programmes["items"].as_array().unwrap().len(), 1);
    assert_eq!(programmes["items"][0]["title"], "Fallback Programme");

    let cursor = channels["next"]
        .as_str()
        .expect("another Channel match remains");
    let continuation = get_json(
        &app.router,
        &format!("/api/v1/search/channels?term=fallback&limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(continuation["items"].as_array().unwrap().len(), 1);
    assert_ne!(continuation["items"][0], channels["items"][0]);
    assert_eq!(continuation["next"], Value::Null);
    assert_eq!(continuation["generation"], channels["generation"]);

    for body in [channels, programmes, continuation] {
        let encoded = body.to_string();
        for canary in [
            CONFIGURATION_CANARY,
            PROVIDER_CANARY,
            EPG_CONFIGURATION_CANARY,
            EPG_PROVIDER_CANARY,
            "source-canary",
            "guide-canary",
            "media.fixture.invalid",
            "fallback-private",
            "https://",
        ] {
            assert!(!encoded.contains(canary), "response leaked {canary}");
        }
    }
}

#[tokio::test]
async fn lane_search_endpoints_inherit_authentication_and_strict_query_contracts() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;

    for uri in [
        "/api/v1/search/channels?term=fallback&limit=1",
        "/api/v1/search/programmes?term=programme&limit=1",
    ] {
        let response = send(&app.router, request(Method::GET, uri, None)).await;
        assert_authentication_required(&response);
    }

    for (uri, field, reason) in [
        (
            "/api/v1/search/channels?term=&limit=1",
            "search-term",
            "required",
        ),
        (
            "/api/v1/search/programmes?term=news&limit=0",
            "page-limit",
            "out-of-range",
        ),
        (
            "/api/v1/search/channels?term=news&limit=nope",
            "page-limit",
            "invalid-format",
        ),
        (
            "/api/v1/search/programmes?term=news&limit=1&cursor=not-a-cursor",
            "page-cursor",
            "invalid-format",
        ),
        (
            "/api/v1/search/channels?term=news&limit=1&unknown=1",
            "query",
            "invalid-format",
        ),
        (
            "/api/v1/search/programmes?term=news&channelLimit=1",
            "query",
            "invalid-format",
        ),
    ] {
        let response = send(&app.router, request(Method::GET, uri, Some(PASSWORD))).await;
        assert_invalid_input(&response, field, reason);
    }

    for uri in [
        "/api/v1/search/channels?limit=1",
        "/api/v1/search/programmes?term=news",
    ] {
        let response = send(&app.router, request(Method::GET, uri, Some(PASSWORD))).await;
        assert_invalid_input(&response, "query", "invalid-format");
    }
}

#[tokio::test]
async fn lane_search_cursors_remain_bound_to_kind_and_term() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;
    let first = get_json(
        &app.router,
        "/api/v1/search/programmes?term=programme&limit=1",
    )
    .await;
    let cursor = first["next"]
        .as_str()
        .expect("another Programme match remains");

    for uri in [
        format!("/api/v1/search/channels?term=programme&limit=1&cursor={cursor}"),
        format!("/api/v1/search/programmes?term=fallback&limit=1&cursor={cursor}"),
    ] {
        let response = send(&app.router, request(Method::GET, &uri, Some(PASSWORD))).await;
        assert_invalid_input(&response, "page-cursor", "cursor-query-mismatch");
        assert!(!response.text.contains(cursor));
    }
}
