package xyz.ponbac.sparrow

import androidx.media3.exoplayer.DecoderCounters
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class NativePlaybackProtocolTest {
  @Test
  fun renderedFrameCounterUsesLiveDecoderCountersWhenProcessingOffsetsAreAbsent() {
    val counter = NativePlaybackFrameCounter()
    val firstDecoder = DecoderCounters()

    counter.onVideoEnabled(firstDecoder)
    firstDecoder.renderedOutputBufferCount = 120
    counter.onVideoFrameProcessingOffset(4)
    assertEquals(120L, counter.value())

    counter.onVideoDisabled(firstDecoder)
    val replacementDecoder = DecoderCounters()
    counter.onVideoEnabled(replacementDecoder)
    replacementDecoder.renderedOutputBufferCount = 30
    assertEquals(150L, counter.value())
  }

  @Test
  fun identityAcceptsOnlyBoundedOpaqueHandles() {
    val identity = NativePlaybackIdentity.parse(
      "play1_0123456789abcdef0123456789abcdef_a",
      "stream1_0123456789abcdef",
    )
    assertNotNull(identity)
    assertNull(
      NativePlaybackIdentity.parse(
        "https://provider.invalid/private.ts?token=canary",
        "stream1_0123456789abcdef",
      ),
    )
    assertNull(
      NativePlaybackIdentity.parse(
        "play1_0123456789abcdef0123456789abcdef_a",
        "stream1_0123456789abcdeF",
      ),
    )
  }

  @Test
  fun viewportAndControlsAreClosedAndBounded() {
    assertNotNull(NativePlaybackViewport.parse(0, 0, 1920, 1080, false))
    assertNull(NativePlaybackViewport.parse(-1, 0, 1920, 1080, false))
    assertNull(NativePlaybackViewport.parse(0, 0, 0, 1080, false))
    assertNull(NativePlaybackViewport.parse(0, 0, 32_769, 1080, true))

    assertNotNull(NativePlaybackControls.parse(0.75f, false, false))
    assertNull(NativePlaybackControls.parse(Float.NaN, false, false))
    assertNull(NativePlaybackControls.parse(1.01f, false, false))
  }

  @Test
  fun startRejectsAReplacementUntilTheExactOwnerStops() {
    val first = identity(1)
    val second = identity(2)

    assertEquals(
      NativePlaybackStartDecision.CREATE,
      nativePlaybackStartDecision(null, first),
    )
    assertEquals(
      NativePlaybackStartDecision.UPDATE,
      nativePlaybackStartDecision(first, first),
    )
    assertEquals(
      NativePlaybackStartDecision.REJECT,
      nativePlaybackStartDecision(first, second),
    )
  }

  @Test
  fun lifecycleFenceRejectsAPrePauseStartEvenAfterAQuickResume() {
    val fence = NativePlaybackLifecycleFence()
    assertNull(fence.startTicket())
    fence.resume()
    val prePause = checkNotNull(fence.startTicket())
    assertTrue(fence.permits(prePause))

    fence.suspend()
    assertNull(fence.startTicket())
    assertFalse(fence.permits(prePause))

    fence.resume()
    assertFalse(
      "resume must not revive a JNI start that was queued before pause",
      fence.permits(prePause),
    )
    val resumed = checkNotNull(fence.startTicket())
    assertTrue(fence.permits(resumed))

    fence.suspend()
    assertFalse(fence.permits(resumed))
  }

  @Test
  fun debugAcceptanceSilenceOverridesEveryRequestedControl() {
    val playing = checkNotNull(NativePlaybackControls.parse(0.8f, false, false))
    val paused = checkNotNull(NativePlaybackControls.parse(0.4f, false, true))

    assertEquals(playing, playing.withForcedSilence(false))
    assertEquals(0.0f, playing.withForcedSilence(true).volume)
    assertTrue(playing.withForcedSilence(true).muted)
    assertFalse(playing.withForcedSilence(true).paused)
    assertEquals(0.0f, paused.withForcedSilence(true).volume)
    assertTrue(paused.withForcedSilence(true).muted)
    assertTrue(paused.withForcedSilence(true).paused)
    assertEquals(
      "xyz.ponbac.sparrow.ACCEPTANCE_SILENT",
      ACCEPTANCE_SILENT_EXTRA,
    )
    assertEquals(1_000L, NATIVE_PLAYBACK_RELEASE_TIMEOUT_MS)
  }

  @Test
  fun timeoutBeforeUiExecutionMakesTheLateRunnableANoOp() {
    val request = NativePlaybackMainThreadRequest(false)
    val executed = AtomicBoolean(false)

    assertFalse(request.await(1, TimeUnit.MILLISECONDS))
    request.execute(operation = {
      executed.set(true)
      true
    })

    assertFalse(executed.get())
  }

  @Test
  fun timeoutDuringUiExecutionRollsBackBeforeTheRunnableReturns() {
    val request = NativePlaybackMainThreadRequest(false)
    val entered = CountDownLatch(1)
    val release = CountDownLatch(1)
    val published = AtomicBoolean(false)
    val rolledBack = AtomicBoolean(false)
    val worker = Thread {
      request.execute(
        operation = {
          entered.countDown()
          assertTrue(release.await(2, TimeUnit.SECONDS))
          published.set(true)
          true
        },
        rollback = { created ->
          if (created) {
            published.set(false)
            rolledBack.set(true)
          }
        },
      )
    }
    worker.start()
    assertTrue(entered.await(2, TimeUnit.SECONDS))

    assertFalse(request.await(1, TimeUnit.MILLISECONDS))
    release.countDown()
    worker.join(2_000)

    assertFalse(worker.isAlive)
    assertFalse(published.get())
    assertTrue(rolledBack.get())
  }

  private fun identity(sequence: Int): NativePlaybackIdentity = checkNotNull(
    NativePlaybackIdentity.parse(
      "play1_0123456789abcdef0123456789abcdef_${sequence.toString(16)}",
      "stream1_${sequence.toString(16).padStart(16, '0')}",
    ),
  )
}
