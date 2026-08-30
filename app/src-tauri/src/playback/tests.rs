use std::{
    collections::VecDeque,
    future::Future,
    io::{Read as _, Write as _},
    net::TcpListener,
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::Stream;
use sparrow_core::{
    ChannelId, ChannelQuery, Clock, CoreAdapters, PageLimit, PageRequest, ResolvedPlaybackSource,
    SourceConfigurationInput, SourceState, SparrowCore, SystemClock,
};
use sparrow_snapshot_store::AtomicFileSnapshotStore;
use sparrow_source_http::{
    HttpSourceAccess, PlaybackAccessError, PlaybackByteStream, PlaybackReadError,
};
use tempfile::TempDir;

use super::*;
use crate::selected_transport_stream::{AudioCodec, AudioSelectionReason, MissingAudioSelection};

const PRIVATE_PLAYBACK_CANARY: &str =
    "http://subscriber:secret@provider.invalid/live.ts?token=private";

#[tokio::test]
async fn pulls_are_bounded_lossless_and_distinguish_empty_chunks_from_eof() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let large = (0..(MAX_NATIVE_PULL_BYTES + 13))
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let access = FakeAccess::new([OpenPlan::Stream(vec![
        StreamStep::Chunk(Bytes::new()),
        StreamStep::Chunk(Bytes::copy_from_slice(&large)),
        StreamStep::Chunk(Bytes::new()),
        StreamStep::Chunk(Bytes::from_static(b"tail")),
    ])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(1), fixture.channel.clone())
        .await
        .expect("playback starts");

    let first = read(&manager, &started).await.expect("first pull succeeds");
    let second = read(&manager, &started)
        .await
        .expect("remainder pull succeeds");
    let third = read(&manager, &started)
        .await
        .expect("empty provider chunk is skipped");
    let eof = read(&manager, &started).await.expect("EOF is projected");

    assert_eq!(first, large[..MAX_NATIVE_PULL_BYTES]);
    assert_eq!(second, large[MAX_NATIVE_PULL_BYTES..]);
    assert_eq!(third, b"tail");
    assert!(eof.is_empty());
    assert!(matches!(
        read(&manager, &started).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.tracker.active(), 0, "EOF releases the body");
}

#[tokio::test]
async fn header_and_body_failures_are_typed_and_release_provider_resources() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let header_access = FakeAccess::new([OpenPlan::Error(PlaybackAccessError::InvalidResponse)]);
    let header_manager =
        PlaybackManager::with_access(Arc::clone(&fixture.core), header_access.clone());
    assert!(matches!(
        header_manager
            .start(session(2), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Access(
            PlaybackAccessError::InvalidResponse
        ))
    ));
    assert_eq!(header_access.tracker.active(), 0);

    let body_access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Error(
        PlaybackReadError::Interrupted,
    )])]);
    let body_manager = PlaybackManager::with_access(Arc::clone(&fixture.core), body_access.clone());
    let started = body_manager
        .start(session(3), fixture.channel.clone())
        .await
        .expect("headers succeed");
    assert!(matches!(
        read(&body_manager, &started).await,
        Err(PlaybackManagerError::Read(PlaybackReadError::Interrupted))
    ));
    assert_eq!(body_access.tracker.active(), 0);
    assert!(matches!(
        read(&body_manager, &started).await,
        Err(PlaybackManagerError::Cancelled)
    ));
}

#[tokio::test]
async fn initial_access_failure_retains_the_pinned_session_for_reopen() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Error(PlaybackAccessError::TimedOut),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"recovered"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let session_id = session(30);

    assert!(matches!(
        manager
            .start(session_id.clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Access(PlaybackAccessError::TimedOut))
    ));
    let pinned = access.source(0);
    assert!(pinned.upgrade().is_some());
    assert_eq!(access.tracker.active(), 0, "failed open releases its lease");

    let reopened = manager
        .reopen(session_id)
        .await
        .expect("the dormant session reopens");
    assert!(Weak::ptr_eq(&pinned, &access.source(1)));
    assert_eq!(access.opened_location(0), access.opened_location(1));
    assert_eq!(
        read(&manager, &reopened)
            .await
            .expect("reopened body reads"),
        b"recovered"
    );
    manager
        .stop(
            reopened.session_id().clone(),
            Some(reopened.stream_handle().clone()),
        )
        .await
        .expect("recovered session stops");
    assert!(pinned.upgrade().is_none());
}

#[tokio::test]
async fn eof_and_read_failure_become_dormant_and_reopen_with_fresh_handles() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![]),
        OpenPlan::Stream(vec![StreamStep::Error(PlaybackReadError::Interrupted)]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"live-edge"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let first = manager
        .start(session(31), fixture.channel.clone())
        .await
        .expect("first transport opens");
    assert!(
        read(&manager, &first)
            .await
            .expect("EOF is returned")
            .is_empty()
    );
    assert_eq!(access.tracker.active(), 0);

    let second = manager
        .reopen(first.session_id().clone())
        .await
        .expect("EOF session reopens");
    assert_ne!(first.stream_handle(), second.stream_handle());
    assert!(matches!(
        read(&manager, &first).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert!(matches!(
        read(&manager, &second).await,
        Err(PlaybackManagerError::Read(PlaybackReadError::Interrupted))
    ));
    assert_eq!(access.tracker.active(), 0);

    let third = manager
        .reopen(second.session_id().clone())
        .await
        .expect("failed session reopens");
    assert_ne!(second.stream_handle(), third.stream_handle());
    assert_eq!(
        read(&manager, &third)
            .await
            .expect("latest transport reads"),
        b"live-edge"
    );
    assert_eq!(access.tracker.max_active(), 1);
}

#[tokio::test]
async fn matching_stop_cancels_pending_headers_before_acknowledging() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::PendingHeaders]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let session_id = session(4);
    let mut start = Box::pin(manager.start(session_id.clone(), fixture.channel.clone()));

    tokio::select! {
        result = &mut start => panic!("pending headers completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.active() == 1) => {}
    }
    manager
        .stop(session_id, None)
        .await
        .expect("matching stop is acknowledged");

    assert!(matches!(start.await, Err(PlaybackManagerError::Cancelled)));
    assert_eq!(
        access.tracker.active(),
        0,
        "stop waits until the pending request is dropped"
    );
}

#[tokio::test]
async fn suspend_cancels_pending_headers_before_ack_and_retains_the_session() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::PendingHeaders,
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"resumed"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let session_id = session(40);
    let mut start = Box::pin(manager.start(session_id.clone(), fixture.channel.clone()));

    tokio::select! {
        result = &mut start => panic!("pending headers completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.active() == 1) => {}
    }
    manager
        .suspend(session_id.clone())
        .await
        .expect("suspend is acknowledged after cancellation");
    assert_eq!(access.tracker.active(), 0);
    assert!(matches!(start.await, Err(PlaybackManagerError::Cancelled)));
    assert!(access.source(0).upgrade().is_some());

    let reopened = manager
        .reopen(session_id)
        .await
        .expect("suspended headers reopen");
    assert_eq!(
        read(&manager, &reopened).await.expect("resumed body reads"),
        b"resumed"
    );
    assert_eq!(access.tracker.max_active(), 1);
}

#[tokio::test]
async fn suspend_cancels_pending_read_before_ack_and_resumes_at_a_fresh_transport() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"live-edge"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let first = manager
        .start(session(41), fixture.channel.clone())
        .await
        .expect("transport opens");
    let mut pending_read = Box::pin(read(&manager, &first));
    tokio::select! {
        result = &mut pending_read => panic!("pending read completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.read_polls() > 0) => {}
    }

    manager
        .suspend(first.session_id().clone())
        .await
        .expect("suspend drops the body before acknowledgement");
    assert_eq!(access.tracker.active(), 0);
    assert!(matches!(
        pending_read.await,
        Err(PlaybackManagerError::Cancelled)
    ));
    manager
        .suspend(first.session_id().clone())
        .await
        .expect("dormant suspend is idempotent");

    let resumed = manager
        .reopen(first.session_id().clone())
        .await
        .expect("resume opens at the live edge");
    assert_ne!(first.stream_handle(), resumed.stream_handle());
    assert!(matches!(
        read(&manager, &first).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(
        read(&manager, &resumed)
            .await
            .expect("fresh transport reads"),
        b"live-edge"
    );
    assert_eq!(access.tracker.max_active(), 1);
}

#[tokio::test]
async fn stop_before_start_is_bounded_and_does_not_poison_later_sessions() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());

    for sequence in 0..=MAX_STOP_TOMBSTONES {
        manager
            .stop(session(sequence), None)
            .await
            .expect("unknown stop remains idempotent");
    }

    let oldest = manager
        .start(session(0), fixture.channel.clone())
        .await
        .expect("the bounded registry evicts its oldest tombstone");
    assert!(matches!(
        manager
            .start(session(MAX_STOP_TOMBSTONES), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.open_count(), 1);
    manager
        .stop(
            oldest.session_id().clone(),
            Some(oldest.stream_handle().clone()),
        )
        .await
        .expect("active playback stops");
}

#[tokio::test]
async fn suspend_before_start_is_bounded_and_late_start_pins_without_opening() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"resumed"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());

    for sequence in 0..=MAX_SUSPEND_INTENTS {
        manager
            .suspend(session(sequence))
            .await
            .expect("unknown suspend remains idempotent");
    }

    let oldest = manager
        .start(session(0), fixture.channel.clone())
        .await
        .expect("the bounded registry evicts its oldest intent");
    assert_eq!(access.open_count(), 1);
    let late_id = session(MAX_SUSPEND_INTENTS);
    assert!(matches!(
        manager
            .start(late_id.clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(
        access.open_count(),
        1,
        "late suspended start resolves but never opens"
    );
    assert_eq!(access.tracker.active(), 0, "replacement drops prior stream");
    assert!(matches!(
        read(&manager, &oldest).await,
        Err(PlaybackManagerError::Cancelled)
    ));

    let reopened = manager
        .reopen(late_id)
        .await
        .expect("the pinned late session can resume");
    assert_eq!(
        read(&manager, &reopened)
            .await
            .expect("resumed session reads"),
        b"resumed"
    );
}

#[tokio::test]
async fn matching_stop_cancels_a_pending_read_and_releases_the_body() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(70), fixture.channel.clone())
        .await
        .expect("playback starts");
    let mut pending_read = Box::pin(read(&manager, &started));

    tokio::select! {
        result = &mut pending_read => panic!("pending read completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.read_polls() > 0) => {}
    }
    manager
        .stop(
            started.session_id().clone(),
            Some(started.stream_handle().clone()),
        )
        .await
        .expect("stop cancels the read");

    assert!(matches!(
        pending_read.await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.tracker.active(), 0);
}

#[tokio::test]
async fn reopen_replaces_pending_headers_only_after_dropping_the_old_request() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::PendingHeaders,
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"fresh"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let session_id = session(71);
    let mut start = Box::pin(manager.start(session_id.clone(), fixture.channel.clone()));
    tokio::select! {
        result = &mut start => panic!("pending headers completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.active() == 1) => {}
    }

    let reopened = manager
        .reopen(session_id)
        .await
        .expect("reopen replaces pending headers");
    assert!(matches!(start.await, Err(PlaybackManagerError::Cancelled)));
    assert_eq!(access.tracker.max_active(), 1);
    assert_eq!(
        read(&manager, &reopened).await.expect("replacement reads"),
        b"fresh"
    );
}

#[tokio::test]
async fn reopen_replaces_streaming_and_reading_transports_without_overlap() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let streaming_access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(
            b"stream-reopen",
        ))]),
    ]);
    let streaming_manager =
        PlaybackManager::with_access(Arc::clone(&fixture.core), streaming_access.clone());
    let first = streaming_manager
        .start(session(72), fixture.channel.clone())
        .await
        .expect("first stream opens");
    let second = streaming_manager
        .reopen(first.session_id().clone())
        .await
        .expect("streaming transport reopens");
    assert_ne!(first.stream_handle(), second.stream_handle());
    assert_eq!(streaming_access.tracker.max_active(), 1);
    assert!(matches!(
        read(&streaming_manager, &first).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(
        read(&streaming_manager, &second)
            .await
            .expect("replacement reads"),
        b"stream-reopen"
    );

    let reading_access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"read-reopen"))]),
    ]);
    let reading_manager =
        PlaybackManager::with_access(Arc::clone(&fixture.core), reading_access.clone());
    let before_read = reading_manager
        .start(session(73), fixture.channel.clone())
        .await
        .expect("read stream opens");
    let mut pending_read = Box::pin(read(&reading_manager, &before_read));
    tokio::select! {
        result = &mut pending_read => panic!("pending read completed unexpectedly: {result:?}"),
        () = wait_until(|| reading_access.tracker.read_polls() > 0) => {}
    }
    let after_read = reading_manager
        .reopen(before_read.session_id().clone())
        .await
        .expect("reopen cancels the pending read");
    assert!(matches!(
        pending_read.await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_ne!(before_read.stream_handle(), after_read.stream_handle());
    assert_eq!(reading_access.tracker.max_active(), 1);
    assert_eq!(
        read(&reading_manager, &after_read)
            .await
            .expect("reading replacement reads"),
        b"read-reopen"
    );
}

#[tokio::test]
async fn replacement_and_stale_commands_never_overlap_or_retarget_streams() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"new"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let first = manager
        .start(session(80), fixture.channel.clone())
        .await
        .expect("first stream starts");
    let second = manager
        .start(session(81), fixture.channel.clone())
        .await
        .expect("replacement stream starts");

    assert_eq!(access.tracker.active(), 1);
    assert_eq!(
        access.tracker.max_active(),
        1,
        "old provider resources are dropped before replacement open"
    );
    assert!(matches!(
        read(&manager, &first).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert!(matches!(
        manager
            .start(second.session_id().clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    manager
        .stop(
            first.session_id().clone(),
            Some(first.stream_handle().clone()),
        )
        .await
        .expect("stale stop is harmless");
    manager
        .suspend(first.session_id().clone())
        .await
        .expect("stale suspend is harmless");
    assert!(matches!(
        manager.reopen(first.session_id().clone()).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert!(matches!(
        manager
            .start(first.session_id().clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.tracker.active(), 1);
    assert!(
        !access.prior_source_alive_on_open(1),
        "replacement drops the old pinned source before opening the new source"
    );
    assert_eq!(
        read(&manager, &second).await.expect("current handle reads"),
        b"new"
    );
    manager
        .stop(
            second.session_id().clone(),
            Some(second.stream_handle().clone()),
        )
        .await
        .expect("current stream stops");
    assert_eq!(access.tracker.active(), 0);
}

#[tokio::test]
async fn dropped_start_callers_cannot_leave_pending_or_queued_opens() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"current"))]),
        OpenPlan::PendingHeaders,
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let current = manager
        .start(session(85), fixture.channel.clone())
        .await
        .expect("current stream starts");

    let (reply, response) = oneshot::channel();
    drop(response);
    manager
        .controls
        .send(ControlCommand::Start {
            session_id: session(86),
            channel_id: fixture.channel.clone(),
            reply,
        })
        .await
        .expect("closed start command queues");
    tokio::task::yield_now().await;
    assert_eq!(access.open_count(), 1);
    assert_eq!(
        read(&manager, &current)
            .await
            .expect("current stream remains"),
        b"current"
    );

    let mut abandoned = Box::pin(manager.start(session(87), fixture.channel.clone()));
    tokio::select! {
        result = &mut abandoned => panic!("pending headers completed unexpectedly: {result:?}"),
        () = wait_until(|| access.open_count() == 2) => {}
    }
    drop(abandoned);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(access.source(1).upgrade().is_none());
}

#[tokio::test]
async fn dropped_reopen_callers_retire_dormant_sessions_and_pending_opens() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![]), OpenPlan::PendingHeaders]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(87), fixture.channel.clone())
        .await
        .expect("stream opens");
    assert!(
        read(&manager, &started)
            .await
            .expect("stream ends")
            .is_empty()
    );
    let pinned = access.source(0);

    let mut abandoned = Box::pin(manager.reopen(started.session_id().clone()));
    tokio::select! {
        result = &mut abandoned => panic!("pending reopen completed unexpectedly: {result:?}"),
        () = wait_until(|| access.open_count() == 2) => {}
    }
    drop(abandoned);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(pinned.upgrade().is_none());
    assert!(matches!(
        manager.reopen(started.session_id().clone()).await,
        Err(PlaybackManagerError::Cancelled)
    ));
}

#[tokio::test]
async fn stop_releases_dormant_sources_after_each_recoverable_failure() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;

    let access_failure = FakeAccess::new([OpenPlan::Error(PlaybackAccessError::Unavailable)]);
    let access_manager =
        PlaybackManager::with_access(Arc::clone(&fixture.core), access_failure.clone());
    let access_id = session(91);
    assert!(matches!(
        access_manager
            .start(access_id.clone(), fixture.channel.clone())
            .await,
        Err(PlaybackManagerError::Access(
            PlaybackAccessError::Unavailable
        ))
    ));
    let access_source = access_failure.source(0);
    access_manager
        .stop(access_id, None)
        .await
        .expect("access-failed session stops");
    assert!(access_source.upgrade().is_none());

    let eof_access = FakeAccess::new([OpenPlan::Stream(vec![])]);
    let eof_manager = PlaybackManager::with_access(Arc::clone(&fixture.core), eof_access.clone());
    let eof = eof_manager
        .start(session(92), fixture.channel.clone())
        .await
        .expect("EOF stream opens");
    assert!(
        read(&eof_manager, &eof)
            .await
            .expect("EOF arrives")
            .is_empty()
    );
    let eof_source = eof_access.source(0);
    eof_manager
        .stop(eof.session_id().clone(), Some(eof.stream_handle().clone()))
        .await
        .expect("EOF session stops");
    assert!(eof_source.upgrade().is_none());

    let read_access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Error(
        PlaybackReadError::Interrupted,
    )])]);
    let read_manager = PlaybackManager::with_access(Arc::clone(&fixture.core), read_access.clone());
    let failed = read_manager
        .start(session(93), fixture.channel.clone())
        .await
        .expect("read-failure stream opens");
    assert!(matches!(
        read(&read_manager, &failed).await,
        Err(PlaybackManagerError::Read(PlaybackReadError::Interrupted))
    ));
    let read_source = read_access.source(0);
    read_manager
        .stop(
            failed.session_id().clone(),
            Some(failed.stream_handle().clone()),
        )
        .await
        .expect("read-failed session stops");
    assert!(read_source.upgrade().is_none());
}

#[tokio::test]
async fn dropping_a_pending_read_cancels_the_body_without_shifting_bytes() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let started = manager
        .start(session(88), fixture.channel.clone())
        .await
        .expect("stream starts");
    let mut abandoned = Box::pin(read(&manager, &started));
    tokio::select! {
        result = &mut abandoned => panic!("pending read completed unexpectedly: {result:?}"),
        () = wait_until(|| access.tracker.read_polls() > 0) => {}
    }

    drop(abandoned);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(matches!(
        read(&manager, &started).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    manager
        .stop(
            started.session_id().clone(),
            Some(started.stream_handle().clone()),
        )
        .await
        .expect("failed Session stops");
}

#[tokio::test]
async fn dropping_the_manager_tears_down_the_actor_and_provider_body() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    manager
        .start(session(90), fixture.channel.clone())
        .await
        .expect("playback starts");
    let pinned_source = access.source(0);
    assert_eq!(access.tracker.active(), 1);
    assert!(pinned_source.upgrade().is_some());

    drop(manager);
    wait_until(|| access.tracker.active() == 0).await;
    assert!(pinned_source.upgrade().is_none());
}

#[tokio::test]
async fn active_transport_defers_background_refresh_then_dormant_reopen_uses_pinned_source() {
    let (source_location, source_server) = m3u_server([
        PRIVATE_PLAYBACK_CANARY,
        "http://replacement.invalid/new.ts?token=rotated",
    ]);
    let clock = Arc::new(ControlledClock::at("2026-08-29T00:00:00Z"));
    let fixture = CoreFixture::from_source_with_clock(&source_location, clock.clone()).await;
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Chunk(Bytes::from_static(b"replacement"))]),
    ]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access.clone());
    let first = manager
        .start(session(100), fixture.channel.clone())
        .await
        .expect("first playback starts");
    let pinned_source = access.source(0);
    let initial_generation = fixture.core.status().generation();
    clock.set("2026-08-29T07:00:00Z");
    wait_until(|| matches!(fixture.core.status().m3u(), SourceState::Deferred { .. })).await;
    assert_eq!(
        access.tracker.active(),
        1,
        "the active transport keeps its playback lease"
    );

    manager
        .suspend(first.session_id().clone())
        .await
        .expect("suspend releases automatic refresh admission");
    assert_eq!(access.tracker.active(), 0);
    wait_until(|| {
        fixture.core.status().generation() != initial_generation
            && matches!(fixture.core.status().m3u(), SourceState::Fresh { .. })
    })
    .await;
    assert_eq!(
        pinned_source
            .upgrade()
            .expect("the dormant session retains its source")
            .location_for_adapter()
            .as_str(),
        PRIVATE_PLAYBACK_CANARY
    );
    let current_channel = only_channel(&fixture.core);
    assert_eq!(current_channel, fixture.channel);

    let reopened = manager
        .reopen(first.session_id().clone())
        .await
        .expect("dormant session reopens its pinned source");
    assert_eq!(access.opened_location(1), PRIVATE_PLAYBACK_CANARY);
    assert!(Weak::ptr_eq(&pinned_source, &access.source(1)));

    let second = manager
        .start(session(101), current_channel)
        .await
        .expect("a new session resolves the refreshed catalog");
    assert_eq!(
        access.opened_location(2),
        "http://replacement.invalid/new.ts?token=rotated"
    );
    assert!(matches!(
        read(&manager, &reopened).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert!(pinned_source.upgrade().is_none());
    assert_eq!(access.tracker.max_active(), 1);
    manager
        .stop(
            second.session_id().clone(),
            Some(second.stream_handle().clone()),
        )
        .await
        .expect("replacement stops");
    source_server.join().expect("source fixture exits");
}

#[tokio::test]
async fn diagnostics_never_expose_provider_or_opaque_identifier_values() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let access = FakeAccess::new([OpenPlan::Error(PlaybackAccessError::Rejected)]);
    let manager = PlaybackManager::with_access(Arc::clone(&fixture.core), access);
    let private_session = session(0xfeed);
    let session_value = private_session.as_str().to_owned();
    let private_handle = NativeStreamHandle::parse("stream1_feedfacefeedface".to_owned())
        .expect("fixture handle parses");
    let handle_value = private_handle.as_str().to_owned();
    let error = manager
        .start(private_session.clone(), fixture.channel.clone())
        .await
        .expect_err("provider rejects playback");
    let diagnostics = format!("{error:?} {error} {private_session:?} {private_handle:?}");

    assert!(!diagnostics.contains(PRIVATE_PLAYBACK_CANARY));
    assert!(!diagnostics.contains("subscriber"));
    assert!(!diagnostics.contains("secret"));
    assert!(!diagnostics.contains(&session_value));
    assert!(!diagnostics.contains(&handle_value));
}

#[tokio::test]
async fn audio_restart_is_handle_safe_serialized_and_persists_visible_fallback() {
    let fixture = CoreFixture::one(PRIVATE_PLAYBACK_CANARY).await;
    let preferences_root = TempDir::new().expect("temporary preference directory");
    let preferences = AudioPreferenceStore::open(preferences_root.path());
    let first_track = audio_track(1);
    let second_track = audio_track(2);
    let selector = Arc::new(FixturePlaybackTransportSelector::new([
        first_track.clone(),
        second_track.clone(),
    ]));
    let access = FakeAccess::new([
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Pending]),
        OpenPlan::Stream(vec![StreamStep::Pending]),
    ]);
    let manager = PlaybackManager::with_access_preferences_and_selector(
        Arc::clone(&fixture.core),
        access.clone(),
        preferences.clone(),
        selector.clone(),
    );

    let started = manager
        .start(session(0xa0), fixture.channel.clone())
        .await
        .expect("initial track opens");
    assert_eq!(started.tracks().len(), 2);
    assert!(matches!(
        started.selection(),
        AudioSelection::Selected {
            track_id,
            reason: AudioSelectionReason::FirstAvailable,
        } if track_id == &first_track
    ));
    assert_eq!(started.preference_status(), None);

    assert!(matches!(
        manager.stop(started.session_id().clone(), None).await,
        Err(PlaybackManagerError::Cancelled)
    ));
    let stale_handle = NativeStreamHandle::parse("stream1_000000000000ffff".to_owned())
        .expect("stale fixture handle parses");
    assert!(matches!(
        manager
            .restart(
                started.session_id().clone(),
                stale_handle,
                PlaybackRestartIntent::SelectAudio(second_track.clone()),
            )
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.open_count(), 1);
    assert_eq!(access.tracker.active(), 1);

    let selected = manager
        .restart(
            started.session_id().clone(),
            started.stream_handle().clone(),
            PlaybackRestartIntent::SelectAudio(second_track.clone()),
        )
        .await
        .expect("matching generation selects a track");
    assert_ne!(selected.stream_handle(), started.stream_handle());
    assert!(matches!(
        selected.selection(),
        AudioSelection::Selected {
            track_id,
            reason: AudioSelectionReason::Requested,
        } if track_id == &second_track
    ));
    assert_eq!(selected.preference_status(), Some(PreferenceStatus::Saved));
    assert_eq!(
        preferences.preference(&fixture.channel),
        Some(second_track.clone())
    );
    assert_eq!(access.tracker.max_active(), 1);

    assert!(matches!(
        manager
            .stop(
                selected.session_id().clone(),
                Some(started.stream_handle().clone()),
            )
            .await,
        Err(PlaybackManagerError::Cancelled)
    ));
    assert_eq!(access.tracker.active(), 1);

    let reopened = manager
        .reopen(selected.session_id().clone())
        .await
        .expect("current selection survives a fresh transport");
    assert!(matches!(
        reopened.selection(),
        AudioSelection::Selected {
            track_id,
            reason: AudioSelectionReason::CurrentSession,
        } if track_id == &second_track
    ));
    assert_eq!(access.tracker.max_active(), 1);
    manager
        .stop(
            reopened.session_id().clone(),
            Some(reopened.stream_handle().clone()),
        )
        .await
        .expect("current generation stops");
    assert_eq!(access.tracker.active(), 0);
    drop(manager);

    let fallback_selector = Arc::new(FixturePlaybackTransportSelector::new([first_track.clone()]));
    let fallback_access = FakeAccess::new([OpenPlan::Stream(vec![StreamStep::Pending])]);
    let fallback_manager = PlaybackManager::with_access_preferences_and_selector(
        Arc::clone(&fixture.core),
        fallback_access,
        AudioPreferenceStore::open(preferences_root.path()),
        fallback_selector,
    );
    let fallback = fallback_manager
        .start(session(0xa1), fixture.channel.clone())
        .await
        .expect("missing saved track falls back visibly");
    assert!(matches!(
        fallback.selection(),
        AudioSelection::Fallback {
            track_id: Some(track_id),
            missing: MissingAudioSelection::SavedPreference,
        } if track_id == &first_track
    ));
    assert_eq!(fallback.preference_status(), None);
    assert_eq!(
        AudioPreferenceStore::open(preferences_root.path()).preference(&fixture.channel),
        Some(second_track),
        "fallback does not silently overwrite the saved preference"
    );
    fallback_manager
        .stop(
            fallback.session_id().clone(),
            Some(fallback.stream_handle().clone()),
        )
        .await
        .expect("fallback stream stops");
}

async fn read(
    manager: &PlaybackManager,
    started: &StartedPlayback,
) -> Result<Vec<u8>, PlaybackManagerError> {
    manager
        .read(
            started.session_id().clone(),
            started.stream_handle().clone(),
        )
        .await
}

fn session(sequence: usize) -> PlaybackSessionId {
    PlaybackSessionId::parse(format!(
        "play1_0123456789abcdef0123456789abcdef_{sequence:x}"
    ))
    .expect("fixture session id is valid")
}

fn audio_track(sequence: u8) -> AudioTrackId {
    AudioTrackId::parse(format!("atrk1_{sequence:032x}")).expect("fixture Audio Track ID is valid")
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actor reaches the expected state");
}

struct CoreFixture {
    core: Arc<SparrowCore>,
    channel: ChannelId,
    _directory: TempDir,
}

impl CoreFixture {
    async fn one(playback_location: &str) -> Self {
        let (source_location, server) = m3u_server([playback_location]);
        let fixture = Self::from_source(&source_location).await;
        server.join().expect("source fixture exits");
        fixture
    }

    async fn from_source(source_location: &str) -> Self {
        Self::from_source_with_clock(source_location, Arc::new(SystemClock)).await
    }

    async fn from_source_with_clock(source_location: &str, clock: Arc<dyn Clock>) -> Self {
        let directory = TempDir::new().expect("temporary snapshot directory");
        let configuration = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
            source_location,
            None::<String>,
        ))
        .expect("fixture Source Configuration parses");
        let source = Arc::new(HttpSourceAccess::new().expect("source adapter opens"));
        let snapshots = Arc::new(
            AtomicFileSnapshotStore::open(directory.path()).expect("snapshot store opens"),
        );
        let core = Arc::new(
            SparrowCore::bootstrap(
                Some(configuration),
                CoreAdapters::new(source, snapshots, clock),
            )
            .await
            .expect("configured core bootstraps"),
        );
        let channel = only_channel(&core);
        Self {
            core,
            channel,
            _directory: directory,
        }
    }
}

struct ControlledClock {
    current: Mutex<DateTime<Utc>>,
    changed: tokio::sync::watch::Sender<DateTime<Utc>>,
}

impl ControlledClock {
    fn at(value: &str) -> Self {
        let current = parse_instant(value);
        let (changed, _) = tokio::sync::watch::channel(current);
        Self {
            current: Mutex::new(current),
            changed,
        }
    }

    fn set(&self, value: &str) {
        let current = parse_instant(value);
        *self.current.lock().expect("clock lock") = current;
        self.changed.send_replace(current);
    }
}

#[async_trait]
impl Clock for ControlledClock {
    fn now(&self) -> DateTime<Utc> {
        *self.current.lock().expect("clock lock")
    }

    async fn wait_until(&self, deadline: DateTime<Utc>) {
        let mut changed = self.changed.subscribe();
        loop {
            if *changed.borrow_and_update() >= deadline {
                return;
            }
            changed.changed().await.expect("test clock remains open");
        }
    }
}

fn parse_instant(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixture time")
        .with_timezone(&Utc)
}

fn only_channel(core: &SparrowCore) -> ChannelId {
    core.list_channels(ChannelQuery::all(PageRequest::new(
        None,
        PageLimit::new(10).expect("valid page limit"),
    )))
    .expect("fixture catalog browses")
    .items()
    .first()
    .expect("fixture has one channel")
    .id()
    .clone()
}

fn m3u_server<'a>(
    playback_locations: impl IntoIterator<Item = &'a str>,
) -> (String, thread::JoinHandle<()>) {
    let playback_locations = playback_locations
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
    let address = listener.local_addr().expect("fixture address exists");
    listener
        .set_nonblocking(true)
        .expect("fixture listener becomes nonblocking");
    let task = thread::spawn(move || {
        for playback_location in playback_locations {
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "source request arrives");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("source fixture accepts: {error}"),
                }
            };
            let mut request = [0_u8; 2048];
            let bytes = stream.read(&mut request).expect("fixture request reads");
            assert!(request[..bytes].starts_with(b"GET /channels.m3u HTTP/1.1\r\n"));
            let body = format!(
                "#EXTM3U\n#EXTINF:-1 tvg-id=\"fixture-one\" group-title=\"News\",World News\n{playback_location}\n"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("fixture response header writes");
            stream
                .write_all(body.as_bytes())
                .expect("fixture body writes");
            stream.flush().expect("fixture response flushes");
        }
    });
    (format!("http://{address}/channels.m3u"), task)
}

#[derive(Clone)]
struct FakeAccess {
    plans: Arc<Mutex<VecDeque<OpenPlan>>>,
    locations: Arc<Mutex<Vec<String>>>,
    sources: Arc<Mutex<Vec<Weak<ResolvedPlaybackSource>>>>,
    prior_source_alive: Arc<Mutex<Vec<bool>>>,
    tracker: Arc<ConnectionTracker>,
}

struct FixturePlaybackTransportSelector {
    available: Vec<AudioTrackId>,
}

impl FixturePlaybackTransportSelector {
    fn new(available: impl IntoIterator<Item = AudioTrackId>) -> Self {
        Self {
            available: available.into_iter().collect(),
        }
    }
}

impl PlaybackTransportSelector for FixturePlaybackTransportSelector {
    fn open(&self, body: PlaybackByteStream, request: SelectionRequest) -> TransportOpenFuture {
        let available = self.available.clone();
        Box::pin(async move {
            let find =
                |candidate: &AudioTrackId| available.iter().position(|track| track == candidate);
            let (selected, selection) = if available.is_empty() {
                (None, AudioSelection::None)
            } else {
                match request {
                    SelectionRequest::Initial { saved: Some(saved) } => match find(&saved) {
                        Some(index) => (
                            Some(index),
                            AudioSelection::Selected {
                                track_id: saved,
                                reason: AudioSelectionReason::SavedPreference,
                            },
                        ),
                        None => (
                            Some(0),
                            AudioSelection::Fallback {
                                track_id: Some(available[0].clone()),
                                missing: MissingAudioSelection::SavedPreference,
                            },
                        ),
                    },
                    SelectionRequest::Initial { saved: None } => (
                        Some(0),
                        AudioSelection::Selected {
                            track_id: available[0].clone(),
                            reason: AudioSelectionReason::FirstAvailable,
                        },
                    ),
                    SelectionRequest::Continue {
                        current: Some(current),
                        ..
                    } if find(&current).is_some() => (
                        find(&current),
                        AudioSelection::Selected {
                            track_id: current,
                            reason: AudioSelectionReason::CurrentSession,
                        },
                    ),
                    SelectionRequest::Continue {
                        saved: Some(saved), ..
                    } => match find(&saved) {
                        Some(index) => (
                            Some(index),
                            AudioSelection::Selected {
                                track_id: saved,
                                reason: AudioSelectionReason::SavedPreference,
                            },
                        ),
                        None => (
                            Some(0),
                            AudioSelection::Fallback {
                                track_id: Some(available[0].clone()),
                                missing: MissingAudioSelection::SavedPreference,
                            },
                        ),
                    },
                    SelectionRequest::Continue { saved: None, .. } => (
                        Some(0),
                        AudioSelection::Selected {
                            track_id: available[0].clone(),
                            reason: AudioSelectionReason::FirstAvailable,
                        },
                    ),
                    SelectionRequest::Requested(requested) => match find(&requested) {
                        Some(index) => (
                            Some(index),
                            AudioSelection::Selected {
                                track_id: requested,
                                reason: AudioSelectionReason::Requested,
                            },
                        ),
                        None => (
                            Some(0),
                            AudioSelection::Fallback {
                                track_id: Some(available[0].clone()),
                                missing: MissingAudioSelection::Requested,
                            },
                        ),
                    },
                }
            };
            let tracks = available
                .into_iter()
                .enumerate()
                .map(|(index, id)| {
                    AudioTrack::fixture(
                        id,
                        (index == 0).then_some("eng"),
                        (index == 0).then_some("Original"),
                        if index == 0 {
                            AudioCodec::AacAdts
                        } else {
                            AudioCodec::Mpeg2Audio
                        },
                        selected == Some(index),
                    )
                })
                .collect();
            Ok(PreparedPlaybackTransport {
                body,
                tracks,
                selection,
            })
        })
    }
}

impl FakeAccess {
    fn new(plans: impl IntoIterator<Item = OpenPlan>) -> Arc<Self> {
        Arc::new(Self {
            plans: Arc::new(Mutex::new(plans.into_iter().collect())),
            locations: Arc::new(Mutex::new(Vec::new())),
            sources: Arc::new(Mutex::new(Vec::new())),
            prior_source_alive: Arc::new(Mutex::new(Vec::new())),
            tracker: Arc::new(ConnectionTracker::default()),
        })
    }

    fn open_count(&self) -> usize {
        self.locations.lock().expect("locations lock").len()
    }

    fn opened_location(&self, index: usize) -> String {
        self.locations.lock().expect("locations lock")[index].clone()
    }

    fn source(&self, index: usize) -> Weak<ResolvedPlaybackSource> {
        self.sources.lock().expect("sources lock")[index].clone()
    }

    fn prior_source_alive_on_open(&self, index: usize) -> bool {
        self.prior_source_alive
            .lock()
            .expect("source liveness lock")[index]
    }
}

impl NativePlaybackAccess for FakeAccess {
    fn open(&self, source: Arc<ResolvedPlaybackSource>) -> AccessOpenFuture {
        let prior_source_alive = self
            .sources
            .lock()
            .expect("sources lock")
            .iter()
            .any(|candidate| candidate.upgrade().is_some());
        self.prior_source_alive
            .lock()
            .expect("source liveness lock")
            .push(prior_source_alive);
        self.locations
            .lock()
            .expect("locations lock")
            .push(source.location_for_adapter().as_str().to_owned());
        self.sources
            .lock()
            .expect("sources lock")
            .push(Arc::downgrade(&source));
        let plan = self
            .plans
            .lock()
            .expect("plans lock")
            .pop_front()
            .expect("a fake open plan exists");
        let guard = self.tracker.acquire();
        match plan {
            OpenPlan::PendingHeaders => Box::pin(PendingHeaders { _guard: guard }),
            OpenPlan::Error(error) => Box::pin(async move {
                drop(guard);
                Err(error)
            }),
            OpenPlan::Stream(steps) => {
                let stream: PlaybackByteStream = Box::pin(FakeStream {
                    steps: steps.into_iter().collect(),
                    guard: Some(guard),
                });
                Box::pin(async move { Ok(stream) })
            }
        }
    }
}

enum OpenPlan {
    PendingHeaders,
    Error(PlaybackAccessError),
    Stream(Vec<StreamStep>),
}

enum StreamStep {
    Chunk(Bytes),
    Error(PlaybackReadError),
    Pending,
}

struct PendingHeaders {
    _guard: ConnectionGuard,
}

impl Future for PendingHeaders {
    type Output = Result<PlaybackByteStream, PlaybackAccessError>;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

struct FakeStream {
    steps: VecDeque<StreamStep>,
    guard: Option<ConnectionGuard>,
}

impl Stream for FakeStream {
    type Item = Result<Bytes, PlaybackReadError>;

    fn poll_next(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if matches!(self.steps.front(), Some(StreamStep::Pending)) {
            self.guard
                .as_ref()
                .expect("live stream owns a guard")
                .tracker
                .read_polls
                .fetch_add(1, Ordering::AcqRel);
            return Poll::Pending;
        }
        Poll::Ready(match self.steps.pop_front() {
            Some(StreamStep::Chunk(bytes)) => Some(Ok(bytes)),
            Some(StreamStep::Error(error)) => Some(Err(error)),
            Some(StreamStep::Pending) => unreachable!("pending step is retained"),
            None => None,
        })
    }
}

#[derive(Default)]
struct ConnectionTracker {
    active: AtomicUsize,
    max_active: AtomicUsize,
    read_polls: AtomicUsize,
}

impl ConnectionTracker {
    fn acquire(self: &Arc<Self>) -> ConnectionGuard {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        ConnectionGuard {
            tracker: Arc::clone(self),
        }
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::Acquire)
    }

    fn read_polls(&self) -> usize {
        self.read_polls.load(Ordering::Acquire)
    }
}

struct ConnectionGuard {
    tracker: Arc<ConnectionTracker>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.tracker.active.fetch_sub(1, Ordering::AcqRel);
    }
}
