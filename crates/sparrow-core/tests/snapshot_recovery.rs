mod support;

use chrono::{DateTime, Utc};
use sparrow_core::{
    ChannelQuery, CoreError, PageLimit, PageRequest, PrivateSourceValidators, SafeFailure,
    ScheduleQuery, SnapshotOperation, SnapshotRecoveryReason, SourceConfigurationInput, SourceKind,
    SourceState, SparrowCore,
};
use support::{MemorySnapshotStore, ScriptedSource, adapters_at};

const M3U: &[u8] = br#"#EXTM3U
#EXTINF:-1 tvg-id="alpha.id" group-title="News",Alpha
https://media.fixture.invalid/alpha?token=private-playback
#EXTINF:-1 tvg-id="beta.id" group-title="Culture",Beta
https://media.fixture.invalid/beta
"#;

const EPG: &[u8] = br#"<tv>
<channel id="alpha.id"><display-name>Alpha</display-name></channel>
<programme start="20260829120000 +0000" stop="20260829130000 +0000" channel="alpha.id">
<title>Recovered Programme</title>
</programme>
</tv>"#;

const SEEDED_AT: &str = "2026-08-29T12:00:00Z";

#[tokio::test]
async fn offline_restart_recovers_independent_snapshots_without_source_access() {
    let snapshots = MemorySnapshotStore::default();
    let validators = PrivateSourceValidators::parse(
        Some("private-etag-canary".to_owned()),
        Some("private-last-modified-canary".to_owned()),
    )
    .expect("fixture validators are valid");
    let online = ScriptedSource::from_bytes(M3U)
        .with_epg_bytes(EPG)
        .with_m3u_validators(validators.clone());
    let seeded = bootstrap(online, snapshots.clone(), true, SEEDED_AT, "primary").await;
    let generation = seeded
        .status()
        .generation()
        .expect("the online catalog is published");
    assert_eq!(snapshots.activation_count(), 2);
    assert_eq!(snapshots.active_validators(SourceKind::M3u), validators);

    let offline_source = ScriptedSource::unavailable();
    let recovered = bootstrap(
        offline_source.clone(),
        snapshots.clone(),
        true,
        "2026-08-29T13:00:00Z",
        "primary",
    )
    .await;

    assert_eq!(offline_source.open_count(), 0);
    assert_eq!(snapshots.activation_count(), 2);
    assert_eq!(recovered.status().generation(), Some(generation));
    assert!(matches!(
        recovered.status().m3u(),
        SourceState::Fresh { .. }
    ));
    assert!(matches!(
        recovered.status().epg(),
        Some(SourceState::Fresh { .. })
    ));
    assert_eq!(channel_names(&recovered), ["Beta", "Alpha"]);
    let alpha = recovered
        .list_channels(all_channels())
        .expect("the recovered catalog is queryable")
        .items()
        .iter()
        .find(|channel| channel.name() == "Alpha")
        .expect("Alpha exists")
        .id()
        .clone();
    let schedule = recovered
        .schedule(ScheduleQuery::new(alpha, PageRequest::first(limit(10))))
        .expect("the recovered EPG is queryable");
    assert_eq!(schedule.items()[0].title(), "Recovered Programme");

    let diagnostic = format!(
        "{:?} {:?}",
        recovered.status(),
        snapshots.active_validators(SourceKind::M3u)
    );
    for private in ["private-etag-canary", "private-last-modified-canary"] {
        assert!(
            !diagnostic.contains(private),
            "private validator leaked: {private}"
        );
    }
}

#[tokio::test]
async fn old_exact_deadline_and_future_snapshots_are_stale_but_remain_usable() {
    for (validated_at, now) in [
        ("2000-01-01T00:00:00Z", "2099-01-01T00:00:00Z"),
        (SEEDED_AT, "2026-08-29T18:00:00Z"),
        ("2100-01-01T00:00:00Z", "2099-01-01T00:00:00Z"),
    ] {
        let snapshots = MemorySnapshotStore::default();
        let _ = bootstrap(
            ScriptedSource::from_bytes(M3U),
            snapshots.clone(),
            false,
            SEEDED_AT,
            "primary",
        )
        .await;
        snapshots.set_active_validated_at(SourceKind::M3u, utc(validated_at));
        let offline = ScriptedSource::unavailable();
        let recovered = bootstrap(offline.clone(), snapshots, false, now, "primary").await;

        assert_eq!(offline.open_count(), 0);
        assert!(matches!(
            recovered.status().m3u(),
            SourceState::Stale {
                validated_at: actual,
                next_attempt_at: Some(next_attempt_at),
            } if *actual == utc(validated_at) && *next_attempt_at == utc(now)
        ));
        assert_eq!(channel_names(&recovered), ["Beta", "Alpha"]);
    }
}

#[tokio::test]
async fn corrupt_active_candidate_falls_back_repairs_and_reports_safe_evidence() {
    let snapshots = MemorySnapshotStore::default();
    let seeded = bootstrap(
        ScriptedSource::from_bytes(M3U),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    let generation = seeded.status().generation();
    snapshots.duplicate_active_as_fallback(SourceKind::M3u);
    snapshots.corrupt_active_payload(SourceKind::M3u);
    let snapshots = snapshots.with_scan_diagnostic(
        SourceKind::M3u,
        SnapshotRecoveryReason::CorruptActivePointer,
    );
    let offline = ScriptedSource::unavailable();

    let recovered = bootstrap(
        offline.clone(),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;

    assert_eq!(offline.open_count(), 0);
    assert_eq!(recovered.status().generation(), generation);
    assert_eq!(snapshots.adoption_count(), 1);
    let status = recovered.status();
    let recovery = status
        .recovery(SourceKind::M3u)
        .expect("fallback recovery evidence is retained");
    assert!(recovery.fallback_adopted());
    assert!(recovery.rejected().iter().any(|failure| matches!(
        failure,
        SafeFailure::SnapshotRecovery {
            kind: SourceKind::M3u,
            reason: SnapshotRecoveryReason::CorruptActivePointer,
        }
    )));
    assert!(recovery.rejected().iter().any(|failure| matches!(
        failure,
        SafeFailure::SnapshotRecovery {
            kind: SourceKind::M3u,
            reason: SnapshotRecoveryReason::LengthMismatch,
        }
    )));

    let second_offline = ScriptedSource::unavailable();
    let second = bootstrap(
        second_offline.clone(),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    assert_eq!(second_offline.open_count(), 0);
    assert_eq!(second.status().generation(), generation);
    assert_eq!(snapshots.adoption_count(), 1);
}

#[tokio::test]
async fn unreadable_active_candidate_falls_back_with_typed_open_failure() {
    let snapshots = MemorySnapshotStore::default();
    let _ = bootstrap(
        ScriptedSource::from_bytes(M3U),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    snapshots.duplicate_active_as_fallback(SourceKind::M3u);
    snapshots.fail_active_open(SourceKind::M3u);
    let offline = ScriptedSource::unavailable();
    let recovered = bootstrap(
        offline.clone(),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;

    assert_eq!(offline.open_count(), 0);
    assert_eq!(snapshots.adoption_count(), 1);
    assert!(
        recovered
            .status()
            .recovery(SourceKind::M3u)
            .expect("open failure is retained")
            .rejected()
            .iter()
            .any(|failure| matches!(
                failure,
                SafeFailure::Snapshot {
                    kind: SourceKind::M3u,
                    operation: SnapshotOperation::OpenCandidate,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn adoption_failure_keeps_the_verified_fallback_published_and_reports_it() {
    let snapshots = MemorySnapshotStore::default();
    let seeded = bootstrap(
        ScriptedSource::from_bytes(M3U),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    let generation = seeded.status().generation();
    snapshots.duplicate_active_as_fallback(SourceKind::M3u);
    snapshots.corrupt_active_payload(SourceKind::M3u);
    snapshots.fail_adoption(sparrow_core::StoreError::Unavailable);
    let offline = ScriptedSource::unavailable();

    let recovered = bootstrap(offline.clone(), snapshots, false, SEEDED_AT, "primary").await;

    assert_eq!(offline.open_count(), 0);
    assert_eq!(recovered.status().generation(), generation);
    assert_eq!(channel_names(&recovered), ["Beta", "Alpha"]);
    let status = recovered.status();
    let recovery = status
        .recovery(SourceKind::M3u)
        .expect("the failed adoption is retained");
    assert!(!recovery.fallback_adopted());
    assert!(recovery.rejected().iter().any(|failure| matches!(
        failure,
        SafeFailure::Snapshot {
            kind: SourceKind::M3u,
            operation: SnapshotOperation::AdoptCandidate,
            reason: sparrow_core::StoreError::Unavailable,
        }
    )));
}

#[tokio::test]
async fn invalid_length_checksum_and_parse_are_rejected_before_network_fallback() {
    assert_rejected_candidate(
        |snapshots| snapshots.set_active_length(SourceKind::M3u, M3U.len() as u64 + 1),
        |failure| {
            matches!(
                failure,
                SafeFailure::SnapshotRecovery {
                    reason: SnapshotRecoveryReason::LengthMismatch,
                    ..
                }
            )
        },
    )
    .await;
    assert_rejected_candidate(
        |snapshots| snapshots.set_active_length(SourceKind::M3u, 128 * 1024 * 1024 + 1),
        |failure| {
            matches!(
                failure,
                SafeFailure::DecodedLimitExceeded {
                    kind: SourceKind::M3u,
                    limit_bytes: 134_217_728,
                }
            )
        },
    )
    .await;
    assert_rejected_candidate(
        |snapshots| snapshots.set_active_checksum(SourceKind::M3u, [0x55; 32]),
        |failure| {
            matches!(
                failure,
                SafeFailure::SnapshotRecovery {
                    reason: SnapshotRecoveryReason::ChecksumMismatch,
                    ..
                }
            )
        },
    )
    .await;
    assert_rejected_candidate(
        |snapshots| {
            snapshots
                .replace_active_payload(SourceKind::M3u, b"#EXTM3U\n#EXTINF:-1,Broken".to_vec());
        },
        |failure| matches!(failure, SafeFailure::InvalidFormat { .. }),
    )
    .await;
}

#[tokio::test]
async fn changed_source_key_never_reuses_an_old_snapshot() {
    let snapshots = MemorySnapshotStore::default();
    let _ = bootstrap(
        ScriptedSource::from_bytes(M3U),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    let offline = ScriptedSource::unavailable();
    let changed = bootstrap(
        offline.clone(),
        snapshots,
        false,
        SEEDED_AT,
        "changed-source",
    )
    .await;

    assert_eq!(offline.open_count_for(SourceKind::M3u), 1);
    assert!(changed.status().generation().is_none());
    assert!(changed.status().recovery(SourceKind::M3u).is_none());
    assert!(matches!(
        changed.list_channels(all_channels()),
        Err(CoreError::CatalogUnavailable { .. })
    ));
}

#[tokio::test]
async fn invalid_epg_snapshot_never_blocks_offline_m3u_recovery() {
    let snapshots = MemorySnapshotStore::default();
    let _ = bootstrap(
        ScriptedSource::from_bytes(M3U).with_epg_bytes(EPG),
        snapshots.clone(),
        true,
        SEEDED_AT,
        "primary",
    )
    .await;
    snapshots.set_active_checksum(SourceKind::Epg, [0x99; 32]);
    let offline = ScriptedSource::unavailable();
    let recovered = bootstrap(offline.clone(), snapshots, true, SEEDED_AT, "primary").await;

    assert_eq!(offline.open_count(), 0);
    assert_eq!(channel_names(&recovered), ["Beta", "Alpha"]);
    assert!(matches!(
        recovered.status().m3u(),
        SourceState::Fresh { .. }
    ));
    assert!(matches!(
        recovered.status().epg(),
        Some(SourceState::Unavailable {
            failure: Some(SafeFailure::SnapshotRecovery {
                reason: SnapshotRecoveryReason::ChecksumMismatch,
                ..
            }),
        })
    ));
    assert!(
        recovered
            .status()
            .recovery(SourceKind::Epg)
            .expect("EPG rejection is retained")
            .rejected()
            .iter()
            .any(|failure| matches!(
                failure,
                SafeFailure::SnapshotRecovery {
                    reason: SnapshotRecoveryReason::ChecksumMismatch,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn missing_epg_snapshot_keeps_an_offline_recovered_m3u_channel_only() {
    let snapshots = MemorySnapshotStore::default();
    let _ = bootstrap(
        ScriptedSource::from_bytes(M3U),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    let offline = ScriptedSource::unavailable();
    let recovered = bootstrap(offline.clone(), snapshots, true, SEEDED_AT, "primary").await;

    assert_eq!(offline.open_count(), 0);
    assert_eq!(channel_names(&recovered), ["Beta", "Alpha"]);
    assert!(matches!(
        recovered.status().epg(),
        Some(SourceState::Unavailable { failure: None })
    ));
}

async fn assert_rejected_candidate(
    mutate: impl FnOnce(&MemorySnapshotStore),
    expected: impl Fn(&SafeFailure) -> bool,
) {
    let snapshots = MemorySnapshotStore::default();
    let _ = bootstrap(
        ScriptedSource::from_bytes(M3U),
        snapshots.clone(),
        false,
        SEEDED_AT,
        "primary",
    )
    .await;
    mutate(&snapshots);
    let offline = ScriptedSource::unavailable();
    let unavailable = bootstrap(offline.clone(), snapshots, false, SEEDED_AT, "primary").await;

    assert_eq!(offline.open_count_for(SourceKind::M3u), 1);
    assert!(unavailable.status().generation().is_none());
    let status = unavailable.status();
    let recovery = status
        .recovery(SourceKind::M3u)
        .expect("candidate rejection is retained");
    assert!(recovery.rejected().iter().any(expected));
}

async fn bootstrap(
    source: ScriptedSource,
    snapshots: MemorySnapshotStore,
    epg: bool,
    now: &str,
    source_name: &str,
) -> SparrowCore {
    let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        format!("https://private-user:private-secret@provider.fixture.invalid/{source_name}.m3u"),
        epg.then(|| {
            "https://guide-user:guide-secret@provider.fixture.invalid/guide.xml".to_owned()
        }),
    ))
    .expect("fixture Source Configuration is valid");
    SparrowCore::bootstrap(Some(configuration), adapters_at(source, snapshots, now))
        .await
        .expect("bootstrap remains usable")
}

fn all_channels() -> ChannelQuery {
    ChannelQuery::all(PageRequest::first(limit(100)))
}

fn channel_names(core: &SparrowCore) -> Vec<String> {
    core.list_channels(all_channels())
        .expect("the Channel Catalog is queryable")
        .items()
        .iter()
        .map(|channel| channel.name().to_owned())
        .collect()
}

fn limit(value: u16) -> PageLimit {
    PageLimit::new(value).expect("fixture page limit is valid")
}

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("fixture timestamp is valid")
        .with_timezone(&Utc)
}
