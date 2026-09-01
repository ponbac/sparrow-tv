use super::*;

#[tokio::test]
async fn guide_window_projects_grouped_channel_rows_and_overlap_boundaries() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;
    let guide = get_json(
        &app.router,
        "/api/v1/guide?startsAt=2026-08-29T07%3A30%3A00Z&endsAt=2026-08-29T10%3A30%3A00Z&channelLimit=100&group=News",
    )
    .await;
    let rows = guide["items"]
        .as_array()
        .expect("the guide response contains rows");
    assert!(rows.iter().all(|row| row["channel"]["group"] == "News"));
    let exact = rows
        .iter()
        .find(|row| row["channel"]["name"] == "Misleading Name")
        .expect("the exact fixture Channel is in the News guide");
    assert_eq!(
        exact["programmes"],
        json!([
            {
                "title": "Earlier & First",
                "titleTruncated": false,
                "startsAt": "2026-08-29T07:00:00Z",
                "endsAt": "2026-08-29T08:00:00Z",
            },
            {
                "title": "Later Programme",
                "titleTruncated": false,
                "startsAt": "2026-08-29T10:00:00Z",
                "endsAt": "2026-08-29T11:00:00Z",
            }
        ])
    );
    assert_eq!(exact["programmesTruncated"], false);

    let touching = get_json(
        &app.router,
        "/api/v1/guide?startsAt=2026-08-29T08%3A00%3A00Z&endsAt=2026-08-29T10%3A00%3A00Z&channelLimit=100&group=News",
    )
    .await;
    let exact_touching = touching["items"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["channel"]["name"] == "Misleading Name")
        })
        .expect("the exact fixture Channel remains in the boundary guide");
    assert_eq!(exact_touching["programmes"], json!([]));

    let encoded = guide.to_string();
    for canary in [
        CONFIGURATION_CANARY,
        PROVIDER_CANARY,
        EPG_CONFIGURATION_CANARY,
        EPG_PROVIDER_CANARY,
        "media.fixture.invalid",
        "exact-private",
        "https://",
    ] {
        assert!(!encoded.contains(canary), "guide response leaked {canary}");
    }
}

#[tokio::test]
async fn guide_window_bounds_titles_and_never_projects_descriptions() {
    let source_title = "é".repeat(sparrow_core::GuideProgramme::MAX_TITLE_BYTES);
    let source_description = "large-guide-description".repeat(1_024);
    let epg = format!(
        r#"<tv><channel id="exact.id"><display-name>Exact</display-name></channel><programme start="20260829070000 +0000" stop="20260829120000 +0000" channel="exact.id"><title>{source_title}</title><desc>{source_description}</desc></programme></tv>"#
    );
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, epg.as_bytes()).await;
    let guide = get_json(
        &app.router,
        "/api/v1/guide?startsAt=2026-08-29T08%3A00%3A00Z&endsAt=2026-08-29T09%3A00%3A00Z&channelLimit=100",
    )
    .await;
    let programme = guide["items"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["channel"]["name"] == "Misleading Name")
        })
        .and_then(|row| row["programmes"].as_array())
        .and_then(|programmes| programmes.first())
        .expect("the oversized-title Programme is projected into the guide");

    assert_eq!(programme["title"], "é".repeat(128));
    assert_eq!(programme["titleTruncated"], true);
    assert!(programme.get("description").is_none());
    assert!(programme.get("channelId").is_none());
    assert!(!guide.to_string().contains(&source_description));

    let search = get_json(
        &app.router,
        "/api/v1/search?term=%C3%A9&channelLimit=1&programmeLimit=1",
    )
    .await;
    let hit = &search["programmes"]["items"][0];
    assert_eq!(hit["title"], "é".repeat(128));
    assert_eq!(hit["titleTruncated"], true);
    assert_eq!(hit["channel"]["name"], "Misleading Name");
    assert!(hit.get("description").is_none());
    assert!(hit.get("channelId").is_none());
    assert!(!search.to_string().contains(&source_description));
}

#[tokio::test]
async fn guide_window_refines_times_and_scopes_continuations_to_the_query() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;
    let first = get_json(
        &app.router,
        "/api/v1/guide?startsAt=2026-08-29T07%3A00%3A00Z&endsAt=2026-08-29T12%3A00%3A00Z&channelLimit=1",
    )
    .await;
    let cursor = first["next"]
        .as_str()
        .expect("the first guide Channel page continues");
    let equivalent = get_json(
        &app.router,
        &format!(
            "/api/v1/guide?startsAt=2026-08-29T09%3A00%3A00%2B02%3A00&endsAt=2026-08-29T14%3A00%3A00%2B02%3A00&channelLimit=1&cursor={cursor}"
        ),
    )
    .await;
    assert_eq!(equivalent["items"].as_array().map(Vec::len), Some(1));
    let mismatched = send(
        &app.router,
        request(
            Method::GET,
            &format!(
                "/api/v1/guide?startsAt=2026-08-29T07%3A00%3A01Z&endsAt=2026-08-29T12%3A00%3A00Z&channelLimit=1&cursor={cursor}"
            ),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_invalid_input(&mismatched, "page-cursor", "cursor-query-mismatch");

    for (uri, field, reason) in [
        (
            "/api/v1/guide?startsAt=not-an-instant&endsAt=2026-08-29T12%3A00%3A00Z&channelLimit=1",
            "guide-starts-at",
            "invalid-format",
        ),
        (
            "/api/v1/guide?startsAt=2026-08-29T07%3A00%3A00Z&endsAt=2026-08-30T07%3A00%3A01Z&channelLimit=1",
            "guide-ends-at",
            "out-of-range",
        ),
    ] {
        let response = send(&app.router, request(Method::GET, uri, Some(PASSWORD))).await;
        assert_invalid_input(&response, field, reason);
    }
    let oversized_start = "x".repeat(65);
    let response = send(
        &app.router,
        request(
            Method::GET,
            &format!(
                "/api/v1/guide?startsAt={oversized_start}&endsAt=2026-08-29T12%3A00%3A00Z&channelLimit=1"
            ),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_invalid_input(&response, "guide-starts-at", "too-long");
}

#[tokio::test]
async fn schedule_and_search_project_the_enriched_core_fixture_exactly() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;
    let channels = get_json(&app.router, "/api/v1/channels?limit=100").await;
    let exact_id = channel_id_named(&channels, "Misleading Name");

    let first_schedule = get_json(
        &app.router,
        &format!("/api/v1/channels/{exact_id}/schedule?limit=1"),
    )
    .await;
    assert_eq!(first_schedule["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        first_schedule["items"][0],
        json!({
            "channelId": exact_id,
            "title": "Earlier & First",
            "description": "A normalized description",
            "startsAt": "2026-08-29T07:00:00Z",
            "endsAt": "2026-08-29T08:00:00Z",
        })
    );
    let schedule_cursor = first_schedule["next"]
        .as_str()
        .expect("the exact schedule has a continuation");
    let second_schedule = get_json(
        &app.router,
        &format!("/api/v1/channels/{exact_id}/schedule?limit=1&cursor={schedule_cursor}"),
    )
    .await;
    assert_eq!(
        second_schedule["items"][0],
        json!({
            "channelId": exact_id,
            "title": "Later Programme",
            "description": null,
            "startsAt": "2026-08-29T10:00:00Z",
            "endsAt": "2026-08-29T11:00:00Z",
        })
    );
    assert_eq!(second_schedule["next"], Value::Null);
    assert_eq!(first_schedule["generation"], second_schedule["generation"]);

    let search = get_json(
        &app.router,
        "/api/v1/search?term=fallback&channelLimit=10&programmeLimit=10",
    )
    .await;
    assert_eq!(search["generation"], first_schedule["generation"]);
    assert_eq!(search["generation"], search["channels"]["generation"]);
    assert_eq!(search["generation"], search["programmes"]["generation"]);
    assert_eq!(search["channels"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(search["programmes"]["items"].as_array().unwrap().len(), 1);
    let fallback_id = channel_id_named(&search["channels"], "FALLBACK One");
    assert_eq!(
        search["programmes"]["items"][0],
        json!({
            "channel": {
                "id": fallback_id,
                "name": "FALLBACK One",
                "group": "News",
            },
            "title": "Fallback Programme",
            "titleTruncated": false,
            "startsAt": "2026-08-29T11:00:00Z",
            "endsAt": "2026-08-29T12:00:00Z",
        })
    );
    assert_eq!(search["channels"]["next"], Value::Null);
    assert_eq!(search["programmes"]["next"], Value::Null);

    for body in [first_schedule, second_schedule, search] {
        let encoded = body.to_string();
        for canary in [
            CONFIGURATION_CANARY,
            PROVIDER_CANARY,
            EPG_CONFIGURATION_CANARY,
            EPG_PROVIDER_CANARY,
            "source-canary",
            "guide-canary",
            "media.fixture.invalid",
            "exact-private",
            "fallback-private",
            "https://",
        ] {
            assert!(!encoded.contains(canary), "response leaked {canary}");
        }
    }

    for uri in [
        format!("/api/v1/channels/{exact_id}/schedule?limit=1"),
        "/api/v1/search?term=fallback&channelLimit=10&programmeLimit=10".to_owned(),
    ] {
        let mut request = request(Method::GET, &uri, Some(PASSWORD));
        request.headers_mut().insert(
            header::ORIGIN,
            "https://attacker.fixture.invalid".parse().unwrap(),
        );
        let response = send(&app.router, request).await;
        assert_eq!(response.status, StatusCode::OK);
        assert_no_cors(&response.headers);
    }
}

#[tokio::test]
async fn accepted_leap_second_never_escapes_the_browser_timestamp_contract() {
    let leap_guide = String::from_utf8(PROGRAMME_EPG.to_vec())
        .expect("the EPG fixture is UTF-8")
        .replace("20260829090000 +0200", "20260829085960 +0200");
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, leap_guide.as_bytes()).await;
    let channels = get_json(&app.router, "/api/v1/channels?limit=100").await;
    let exact_id = channel_id_named(&channels, "Misleading Name");

    let schedule = get_json(
        &app.router,
        &format!("/api/v1/channels/{exact_id}/schedule?limit=10"),
    )
    .await;

    assert_eq!(schedule["items"][0]["startsAt"], "2026-08-29T07:00:00Z");
    assert!(!schedule.to_string().contains(":60"));
}

#[tokio::test]
async fn channel_only_catalog_has_empty_programme_pages_but_searches_channels() {
    let app = TestApp::fixture(PROGRAMME_M3U).await;
    let channels = get_json(&app.router, "/api/v1/channels?limit=100").await;
    let exact_id = channel_id_named(&channels, "Misleading Name");

    let schedule = get_json(
        &app.router,
        &format!("/api/v1/channels/{exact_id}/schedule?limit=10"),
    )
    .await;
    assert_eq!(schedule["items"], json!([]));
    assert_eq!(schedule["next"], Value::Null);

    let search = get_json(
        &app.router,
        "/api/v1/search?term=fallback&channelLimit=10&programmeLimit=10",
    )
    .await;
    assert_eq!(search["generation"], search["channels"]["generation"]);
    assert_eq!(search["generation"], search["programmes"]["generation"]);
    assert_eq!(search["channels"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(search["programmes"]["items"], json!([]));
    assert_eq!(search["programmes"]["next"], Value::Null);
}

#[tokio::test]
async fn failed_guide_states_keep_channel_search_and_validated_programmes_safe() {
    let first_failure_source = FixtureSource::available(PROGRAMME_M3U);
    let first_failure = TestApp::with_core(
        configured_core_with_configuration(
            first_failure_source,
            Arc::new(MemorySnapshotStore::default()),
            source_configuration_with_epg(),
        )
        .await,
    );
    let first_status = get_json(&first_failure.router, "/api/v1/status").await;
    assert_eq!(first_status["epg"]["_tag"], "failed");
    assert_eq!(first_status["epg"]["validatedAt"], Value::Null);
    assert_eq!(
        first_status["epg"]["failure"],
        json!({
            "_tag": "source-access",
            "source": "epg",
            "reason": "unavailable",
            "retryAfterSeconds": null,
        })
    );
    let first_search = get_json(
        &first_failure.router,
        "/api/v1/search?term=fallback&channelLimit=10&programmeLimit=10",
    )
    .await;
    assert_eq!(
        first_search["channels"]["items"].as_array().unwrap().len(),
        2
    );
    assert_eq!(first_search["programmes"]["items"], json!([]));

    let retained_source = FixtureSource::available_with_epg(PROGRAMME_M3U, PROGRAMME_EPG);
    let retained_core = configured_core_with_configuration(
        retained_source.clone(),
        Arc::new(MemorySnapshotStore::default()),
        source_configuration_with_epg(),
    )
    .await;
    retained_source.set_source_unavailable(SourceKind::Epg);
    retained_core.refresh(RefreshTrigger::Manual).await;
    let retained = TestApp::with_core(retained_core);
    let retained_status = get_json(&retained.router, "/api/v1/status").await;
    assert_eq!(retained_status["epg"]["_tag"], "failed");
    assert!(retained_status["epg"]["validatedAt"].is_string());
    assert_eq!(
        retained_status["epg"]["failure"],
        json!({
            "_tag": "source-access",
            "source": "epg",
            "reason": "unavailable",
            "retryAfterSeconds": null,
        })
    );

    let channels = get_json(&retained.router, "/api/v1/channels?limit=100").await;
    let exact_id = channel_id_named(&channels, "Misleading Name");
    let retained_schedule = get_json(
        &retained.router,
        &format!("/api/v1/channels/{exact_id}/schedule?limit=10"),
    )
    .await;
    assert_eq!(retained_schedule["items"].as_array().unwrap().len(), 2);
    let retained_search = get_json(
        &retained.router,
        "/api/v1/search?term=programme&channelLimit=10&programmeLimit=10",
    )
    .await;
    assert_eq!(
        retained_search["programmes"]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    for canary in [EPG_CONFIGURATION_CANARY, EPG_PROVIDER_CANARY] {
        assert!(!retained_status.to_string().contains(canary));
        assert!(!retained_schedule.to_string().contains(canary));
        assert!(!retained_search.to_string().contains(canary));
    }
}

#[tokio::test]
async fn programme_cursors_are_paginated_scoped_and_stale_safe() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;
    let channels = get_json(&app.router, "/api/v1/channels?limit=100").await;
    let exact_id = channel_id_named(&channels, "Misleading Name");
    let fallback_id = channel_id_named(&channels, "FALLBACK One");

    let schedule = get_json(
        &app.router,
        &format!("/api/v1/channels/{exact_id}/schedule?limit=1"),
    )
    .await;
    let schedule_cursor = schedule["next"]
        .as_str()
        .expect("the exact schedule continues");
    let mismatched_schedule = send(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/channels/{fallback_id}/schedule?limit=1&cursor={schedule_cursor}"),
            Some(PASSWORD),
        ),
    )
    .await;
    assert_invalid_input(&mismatched_schedule, "page-cursor", "cursor-query-mismatch");

    let search = get_json(
        &app.router,
        "/api/v1/search?term=programme&channelLimit=1&programmeLimit=1",
    )
    .await;
    assert_eq!(search["channels"]["items"], json!([]));
    let programme_cursor = search["programmes"]["next"]
        .as_str()
        .expect("another Programme search match remains");
    let second_search = get_json(
        &app.router,
        &format!(
            "/api/v1/search?term=programme&channelLimit=1&programmeLimit=1&programmeCursor={programme_cursor}"
        ),
    )
    .await;
    assert_eq!(
        second_search["programmes"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(second_search["programmes"]["next"], Value::Null);

    let channel_search = get_json(
        &app.router,
        "/api/v1/search?term=fallback&channelLimit=1&programmeLimit=1",
    )
    .await;
    assert_eq!(
        channel_search["channels"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let channel_cursor = channel_search["channels"]["next"]
        .as_str()
        .expect("another Channel search match remains");
    let second_channel_search = get_json(
        &app.router,
        &format!(
            "/api/v1/search?term=fallback&channelLimit=1&channelCursor={channel_cursor}&programmeLimit=1"
        ),
    )
    .await;
    assert_eq!(
        second_channel_search["channels"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(second_channel_search["channels"]["next"], Value::Null);

    for uri in [
        format!(
            "/api/v1/search?term=programme&channelLimit=1&channelCursor={programme_cursor}&programmeLimit=1"
        ),
        format!(
            "/api/v1/search?term=fallback&channelLimit=1&programmeLimit=1&programmeCursor={programme_cursor}"
        ),
    ] {
        let response = send(&app.router, request(Method::GET, &uri, Some(PASSWORD))).await;
        assert_invalid_input(&response, "page-cursor", "cursor-query-mismatch");
    }

    let changed_epg = String::from_utf8(PROGRAMME_EPG.to_vec())
        .expect("the EPG fixture is UTF-8")
        .replace("Later Programme", "Changed Later Programme");
    let changed = TestApp::fixture_with_guide(PROGRAMME_M3U, changed_epg.as_bytes()).await;
    for uri in [
        format!("/api/v1/channels/{exact_id}/schedule?limit=1&cursor={schedule_cursor}"),
        format!(
            "/api/v1/search?term=programme&channelLimit=1&programmeLimit=1&programmeCursor={programme_cursor}"
        ),
    ] {
        let response = send(&changed.router, request(Method::GET, &uri, Some(PASSWORD))).await;
        assert_eq!(response.status, StatusCode::CONFLICT);
        assert_eq!(response.json["error"]["_tag"], "stale-cursor");
        assert_eq!(
            response.json["error"]["current"],
            changed.core.status().generation().unwrap().get()
        );
        assert!(!response.text.contains(schedule_cursor));
        assert!(!response.text.contains(programme_cursor));
    }
}

#[tokio::test]
async fn programme_queries_reject_unknown_missing_and_unrefined_input() {
    let app = TestApp::fixture_with_guide(PROGRAMME_M3U, PROGRAMME_EPG).await;
    let channels = get_json(&app.router, "/api/v1/channels?limit=100").await;
    let exact_id = channel_id_named(&channels, "Misleading Name");
    let oversized_term = "x".repeat(1025);
    let cases = [
        (
            format!("/api/v1/channels/{exact_id}/schedule?limit=0"),
            "page-limit",
            "out-of-range",
        ),
        (
            format!("/api/v1/channels/{exact_id}/schedule?limit=nope"),
            "page-limit",
            "invalid-format",
        ),
        (
            format!("/api/v1/channels/{exact_id}/schedule?cursor=not-a-cursor"),
            "page-cursor",
            "invalid-format",
        ),
        (
            format!("/api/v1/channels/{exact_id}/schedule?unknown=1"),
            "query",
            "invalid-format",
        ),
        (
            "/api/v1/search?term=&channelLimit=10&programmeLimit=10".to_owned(),
            "search-term",
            "required",
        ),
        (
            format!("/api/v1/search?term={oversized_term}&channelLimit=10&programmeLimit=10"),
            "search-term",
            "too-long",
        ),
        (
            "/api/v1/search?term=news&channelLimit=0&programmeLimit=10".to_owned(),
            "page-limit",
            "out-of-range",
        ),
        (
            "/api/v1/search?term=news&channelLimit=10&programmeLimit=101".to_owned(),
            "page-limit",
            "out-of-range",
        ),
        (
            "/api/v1/search?term=news&channelLimit=nope&programmeLimit=10".to_owned(),
            "page-limit",
            "invalid-format",
        ),
        (
            "/api/v1/search?term=news&channelLimit=10&channelCursor=not-a-cursor&programmeLimit=10"
                .to_owned(),
            "page-cursor",
            "invalid-format",
        ),
        (
            "/api/v1/search?term=news&channel_limit=10&programmeLimit=10".to_owned(),
            "query",
            "invalid-format",
        ),
        (
            "/api/v1/search?term=news&channelLimit=10&programmeLimit=10&unknown=1".to_owned(),
            "query",
            "invalid-format",
        ),
    ];

    for (uri, field, reason) in cases {
        let response = send(&app.router, request(Method::GET, &uri, Some(PASSWORD))).await;
        assert_invalid_input(&response, field, reason);
    }

    for missing in [
        "/api/v1/search?channelLimit=10&programmeLimit=10",
        "/api/v1/search?term=news&programmeLimit=10",
        "/api/v1/search?term=news&channelLimit=10",
    ] {
        let response = send(&app.router, request(Method::GET, missing, Some(PASSWORD))).await;
        assert_invalid_input(&response, "query", "invalid-format");
    }
}
