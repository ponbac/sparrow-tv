use std::{
    collections::HashSet,
    fs,
    future::{Future, poll_fn},
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    sync::Arc,
    task::Poll,
    time::Duration,
};

use serde_json::json;
use sparrow_core::ChannelQuality;
use sparrow_installed_catalog::{
    CatalogStartupError, GuideBackdrop, GuidePreferenceChange, GuidePreferenceError,
    InstalledCatalog,
};
use tempfile::tempdir;

const FAMILY: &str = r#"["News \"East\"","evening report"]"#;

#[tokio::test]
async fn guide_preferences_round_trip_offline_with_webview_v1_fields() {
    let directory = tempdir().unwrap();
    let catalog = open(directory.path()).await;
    let mut changes = catalog.subscribe_guide_preferences();
    let initial = changes.borrow_and_update().clone();
    assert!(initial.excluded_groups().is_empty());
    assert!(initial.variant_preferences().is_empty());

    assert_eq!(initial.backdrop(), GuideBackdrop::NordicGradient);
    let groups = HashSet::from(["".to_owned(), "News \"East\"".to_owned()]);
    let saved = catalog
        .update_guide_preferences(vec![
            visibility("", true),
            visibility("News \"East\"", true),
            prefer(ChannelQuality::Fhd),
            GuidePreferenceChange::SetBackdrop(GuideBackdrop::ChannelTint),
        ])
        .await
        .unwrap();
    assert!(changes.has_changed().unwrap());
    assert!(Arc::ptr_eq(&saved, &changes.borrow_and_update()));
    assert!(initial.excluded_groups().is_empty());
    let unchanged = catalog
        .update_guide_preferences(vec![prefer(ChannelQuality::Fhd)])
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&saved, &unchanged));
    assert!(!changes.has_changed().unwrap());

    let record = catalog.private_root().join("guide-preferences.json");
    assert_eq!(
        fs::metadata(&record).unwrap().permissions().mode() & 0o777,
        0o600,
    );
    let encoded: serde_json::Value = serde_json::from_slice(&fs::read(record).unwrap()).unwrap();
    assert_eq!(
        encoded,
        json!({
            "version": 1,
            "sparrow.backdrop:v1": "channel-tint",
            "sparrow.board-group-exclusions:v1": { "excluded": ["", "News \"East\""] },
            "sparrow.variant-preferences:v1": {
                "preferences": [{ "family": FAMILY, "quality": "fhd" }],
            },
        }),
    );
    drop(catalog);

    let offline = open(directory.path()).await;
    assert!(!offline.core().status().configuration().is_configured());
    assert!(offline.guide_preferences().excluded_groups().is_empty());
    let mut reloaded = offline.subscribe_guide_preferences();
    let recovered = offline.load_guide_preferences().await.unwrap();
    assert!(reloaded.has_changed().unwrap());
    assert!(Arc::ptr_eq(&recovered, &reloaded.borrow_and_update()));
    assert_eq!(recovered.backdrop(), GuideBackdrop::ChannelTint);
    assert_eq!(recovered.excluded_groups(), &groups);
    assert_eq!(
        recovered.variant_preferences().get(FAMILY),
        Some(&ChannelQuality::Fhd)
    );
    assert_eq!(
        recovered.variant_choices().collect::<Vec<_>>(),
        [(
            "News \"East\"".to_owned(),
            "evening report".to_owned(),
            ChannelQuality::Fhd
        )]
    );
}

#[tokio::test]
async fn future_backdrop_preserves_other_guide_preferences() {
    let directory = tempdir().unwrap();
    let catalog = open(directory.path()).await;
    catalog
        .update_guide_preferences(vec![prefer(ChannelQuality::Hd), visibility("Local", true)])
        .await
        .unwrap();
    let path = catalog.private_root().join("guide-preferences.json");
    drop(catalog);
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["sparrow.backdrop:v1"] = json!("future-backdrop");
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();

    let catalog = open(directory.path()).await;
    let preferences = catalog.load_guide_preferences().await.unwrap();
    assert_eq!(preferences.backdrop(), GuideBackdrop::NordicGradient);
    assert!(preferences.excluded_groups().contains("Local"));
    assert_eq!(
        preferences.variant_preferences().get(FAMILY),
        Some(&ChannelQuality::Hd)
    );
}

#[tokio::test]
async fn concurrent_visibility_choices_compose_and_show_all_respects_batch_order() {
    let directory = tempdir().unwrap();
    let catalog = open(directory.path()).await;
    let (local, world) = tokio::join!(
        catalog.update_guide_preferences(vec![visibility("Local", true)]),
        catalog.update_guide_preferences(vec![visibility("World", true)]),
    );
    local.unwrap();
    world.unwrap();
    let both = catalog.guide_preferences();
    assert_eq!(
        both.excluded_groups(),
        &HashSet::from(["Local".to_owned(), "World".to_owned()]),
    );
    let unchanged = catalog
        .update_guide_preferences(vec![visibility("World", true)])
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&both, &unchanged));

    let visible = catalog
        .update_guide_preferences(vec![visibility("Local", false)])
        .await
        .unwrap();
    assert_eq!(
        visible.excluded_groups(),
        &HashSet::from(["World".to_owned()])
    );

    let selected = catalog
        .update_guide_preferences(vec![
            GuidePreferenceChange::ShowAllGroups,
            visibility("", true),
        ])
        .await
        .unwrap();
    assert_eq!(selected.excluded_groups(), &HashSet::from([String::new()]));
}

#[test]
fn cancelled_waiter_does_not_undo_the_admitted_write_or_reorder_later_changes() {
    let directory = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let catalog = open(directory.path()).await;
        let mut changes = catalog.subscribe_guide_preferences();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
        });
        waiting.await.unwrap();

        let groups = HashSet::from(["News".to_owned()]);
        let mut abandoned =
            Box::pin(catalog.update_guide_preferences(vec![
                visibility("News", true),
                prefer(ChannelQuality::Hd),
            ]));
        assert!(poll_fn(|cx| Poll::Ready(abandoned.as_mut().poll(cx).is_pending())).await);
        drop(abandoned);
        assert!(!changes.has_changed().unwrap());

        let mut second =
            Box::pin(catalog.update_guide_preferences(vec![prefer(ChannelQuality::Uhd)]));
        assert!(poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx).is_pending())).await);
        release.send(()).unwrap();
        occupied.await.unwrap();
        let saved = tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.excluded_groups(), &groups);
        assert_eq!(
            saved.variant_preferences().get(FAMILY),
            Some(&ChannelQuality::Uhd)
        );
        assert!(Arc::ptr_eq(&saved, &changes.borrow_and_update()));
    });
}

#[test]
fn abandoned_preference_write_retains_the_instance_until_it_finishes() {
    let directory = tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let catalog = open(directory.path()).await;
        let mut changes = catalog.subscribe_guide_preferences();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
        });
        waiting.await.unwrap();

        let mut abandoned =
            Box::pin(catalog.update_guide_preferences(vec![prefer(ChannelQuality::Hd)]));
        assert!(poll_fn(|cx| Poll::Ready(abandoned.as_mut().poll(cx).is_pending())).await);
        drop(abandoned);
        drop(catalog);

        let locked = InstalledCatalog::prepare(directory.path()).err();
        release.send(()).unwrap();
        occupied.await.unwrap();
        assert_eq!(locked, Some(CatalogStartupError::AlreadyRunning));
        tokio::time::timeout(Duration::from_secs(5), changes.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            changes
                .borrow_and_update()
                .variant_preferences()
                .get(FAMILY),
            Some(&ChannelQuality::Hd),
        );
    });
}

#[tokio::test]
async fn failed_save_keeps_the_last_durable_snapshot_and_can_retry() {
    let directory = tempdir().unwrap();
    let catalog = open(directory.path()).await;
    let previous = catalog
        .update_guide_preferences(vec![prefer(ChannelQuality::Sd)])
        .await
        .unwrap();
    let mut changes = catalog.subscribe_guide_preferences();
    let file = catalog.private_root().join("guide-preferences.json");
    let before = fs::read(&file).unwrap();
    let pending = catalog.private_root().join(".guide-preferences.pending");
    symlink(&file, &pending).unwrap();

    let failed = catalog
        .update_guide_preferences(vec![prefer(ChannelQuality::Fhd)])
        .await;
    assert_eq!(failed.err(), Some(GuidePreferenceError::UnsafeLayout));
    assert!(!changes.has_changed().unwrap());
    assert!(Arc::ptr_eq(&previous, &catalog.guide_preferences()));
    assert_eq!(fs::read(file).unwrap(), before);

    fs::remove_file(pending).unwrap();
    let recovered = catalog
        .update_guide_preferences(vec![prefer(ChannelQuality::Fhd)])
        .await
        .unwrap();
    assert!(changes.has_changed().unwrap());
    assert!(Arc::ptr_eq(&recovered, &changes.borrow_and_update()));
}

#[tokio::test]
async fn unreadable_record_does_not_prevent_a_later_lazy_load() {
    let directory = tempdir().unwrap();
    let first = open(directory.path()).await;
    first
        .update_guide_preferences(vec![prefer(ChannelQuality::Hd)])
        .await
        .unwrap();
    let file = first.private_root().join("guide-preferences.json");
    drop(first);

    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let catalog = open(directory.path()).await;
    let failed = catalog.load_guide_preferences().await;
    assert_eq!(failed.err(), Some(GuidePreferenceError::UnsafeLayout));
    assert!(catalog.guide_preferences().variant_preferences().is_empty());

    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let recovered = catalog.load_guide_preferences().await.unwrap();
    assert_eq!(
        recovered.variant_preferences().get(FAMILY),
        Some(&ChannelQuality::Hd),
    );
}

#[tokio::test]
async fn corrupt_preferences_recover_lazily_and_invalid_batches_publish_nothing() {
    let directory = tempdir().unwrap();
    let catalog = open(directory.path()).await;
    let file = catalog.private_root().join("guide-preferences.json");
    fs::write(
        &file,
        r#"{"version":1,"sparrow.board-group-exclusions:v1":{"excluded":[42]}}"#,
    )
    .unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();

    let initial = catalog.load_guide_preferences().await.unwrap();
    assert!(initial.excluded_groups().is_empty());
    assert!(initial.variant_preferences().is_empty());
    let changes = catalog.subscribe_guide_preferences();
    let failed = catalog
        .update_guide_preferences(vec![
            prefer(ChannelQuality::Hd),
            visibility("X\u{7f}", true),
        ])
        .await;
    assert_eq!(failed.err(), Some(GuidePreferenceError::Invalid));
    assert!(!changes.has_changed().unwrap());
    assert!(Arc::ptr_eq(&initial, &catalog.guide_preferences()));

    let saved = catalog
        .update_guide_preferences(vec![prefer(ChannelQuality::Hd)])
        .await
        .unwrap();
    assert_eq!(
        saved.variant_preferences().get(FAMILY),
        Some(&ChannelQuality::Hd)
    );
}

async fn open(path: &Path) -> Arc<InstalledCatalog> {
    InstalledCatalog::prepare(path)
        .unwrap()
        .finish_open()
        .await
        .unwrap()
}

fn prefer(quality: ChannelQuality) -> GuidePreferenceChange {
    GuidePreferenceChange::PreferVariant {
        group: "News \"East\"".to_owned(),
        normalized_title: "evening report".to_owned(),
        quality,
    }
}

fn visibility(name: &str, hidden: bool) -> GuidePreferenceChange {
    GuidePreferenceChange::SetGroupVisibility {
        name: name.to_owned(),
        hidden,
    }
}
