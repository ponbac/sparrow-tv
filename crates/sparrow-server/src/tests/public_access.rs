use super::*;

#[tokio::test]
async fn public_spa_and_catalog_need_no_login_or_password_query() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG)
        .await
        .into_public();

    for uri in ["/app", "/app/", "/app/catalog/deep-link"] {
        let response = send(&app.router, request(Method::GET, uri, None)).await;
        assert_eq!(response.status, StatusCode::OK, "{uri}");
        assert!(response.text.contains("Sparrow fixture app"));
        assert!(!response.headers.contains_key(header::WWW_AUTHENTICATE));
    }
    for uri in [
        "/api/v1/capabilities",
        "/api/v1/status",
        "/api/v1/groups?limit=10",
        "/api/v1/channels?limit=10",
        "/api/v1/guide?startsAt=2026-08-29T07%3A30%3A00Z&endsAt=2026-08-29T10%3A30%3A00Z&channelLimit=10",
        "/api/v1/search?term=news&channelLimit=10&programmeLimit=10",
        "/api/v1/search/channels?term=news&limit=10",
        "/api/v1/search/programmes?term=news&limit=10",
    ] {
        let response = send(&app.router, request(Method::GET, uri, None)).await;
        assert_eq!(response.status, StatusCode::OK, "{uri}: {}", response.text);
        assert_public_redaction(&response);
    }
    let status = send(&app.router, request(Method::GET, "/api/v1/status", None)).await;
    assert_eq!(
        status.json["configuration"],
        json!({ "configured": true, "epgConfigured": true })
    );
    let channels = send(
        &app.router,
        request(Method::GET, "/api/v1/channels?limit=10", None),
    )
    .await;
    let id = channel_id_named(&channels.json, "Misleading Name");
    for uri in [
        format!("/api/v1/channels/{id}"),
        format!("/api/v1/channels/{id}/schedule?limit=10"),
    ] {
        let response = send(&app.router, request(Method::GET, &uri, None)).await;
        assert_eq!(response.status, StatusCode::OK, "{uri}");
        assert_public_redaction(&response);
    }

    // Stale browser credentials do not re-enable authentication in public mode.
    let mut stale = request(Method::GET, "/api/v1/status", None);
    stale
        .headers_mut()
        .insert(header::AUTHORIZATION, "Basic not-base64!".parse().unwrap());
    let stale = send(&app.router, stale).await;
    assert_eq!(stale.status, StatusCode::OK);
    assert_public_redaction(&stale);
}

#[tokio::test]
async fn public_source_configuration_stays_deployment_readonly() {
    let app = TestApp::fixture(BROWSE_M3U).await.into_public();
    let capabilities = send(
        &app.router,
        request(Method::GET, "/api/v1/capabilities", None),
    )
    .await;
    assert_eq!(capabilities.status, StatusCode::OK);
    assert_eq!(
        capabilities.json["sourceConfiguration"],
        "deployment-readonly"
    );
    let generation = app.core.status().generation();

    for path in [
        "/api/v1/configuration",
        "/api/v1/source-configuration",
        "/api/v1/sources",
    ] {
        for method in [
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ] {
            let response = send(
                &app.router,
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"m3u":"https://untrusted.fixture.invalid/source"}"#,
                    ))
                    .unwrap(),
            )
            .await;
            assert_invalid_input(&response, "route", "invalid-format");
            assert_public_redaction(&response);
            assert!(!response.text.contains("untrusted.fixture.invalid"));
        }
    }
    assert_eq!(app.core.status().generation(), generation);
}

fn assert_public_redaction(response: &ObservedResponse) {
    assert!(!response.headers.contains_key(header::WWW_AUTHENTICATE));
    assert_no_cors(&response.headers);
    for marker in [
        PASSWORD,
        CONFIGURATION_CANARY,
        PROVIDER_CANARY,
        PLAYBACK_CANARY,
        EPG_CONFIGURATION_CANARY,
        EPG_PROVIDER_CANARY,
        "source-canary",
        "guide-canary",
        "https://",
    ] {
        assert!(
            !response.text.contains(marker),
            "private marker leaked: {marker}"
        );
        for value in response.headers.values() {
            assert!(!value.to_str().unwrap().contains(marker));
        }
    }
}
