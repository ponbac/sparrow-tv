package xyz.ponbac.sparrow

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

internal const val ACCEPTANCE_SILENT_EXTRA = "xyz.ponbac.sparrow.ACCEPTANCE_SILENT"
internal const val NATIVE_PLAYBACK_RELEASE_TIMEOUT_MS = 1_000L

@ConsistentCopyVisibility
internal data class NativePlaybackIdentity private constructor(
  val sessionId: String,
  val streamHandle: String,
) {
  companion object {
    private val sessionPattern = Regex("^play1_[0-9a-f]{32}_[0-9a-f]+$")
    private val streamPattern = Regex("^stream1_[0-9a-f]{16}$")

    fun isValidSessionId(sessionId: String): Boolean =
      sessionId.length <= 64 && sessionPattern.matches(sessionId)

    fun parse(sessionId: String, streamHandle: String): NativePlaybackIdentity? {
      if (
        !isValidSessionId(sessionId) ||
          !streamPattern.matches(streamHandle)
      ) {
        return null
      }
      return NativePlaybackIdentity(sessionId, streamHandle)
    }
  }
}

@ConsistentCopyVisibility
internal data class NativePlaybackViewport private constructor(
  val left: Int,
  val top: Int,
  val width: Int,
  val height: Int,
  val fullscreen: Boolean,
) {
  companion object {
    private const val MAX_COORDINATE = 32_768
    private const val MAX_EXTENT = 32_768

    fun parse(
      left: Int,
      top: Int,
      width: Int,
      height: Int,
      fullscreen: Boolean,
    ): NativePlaybackViewport? {
      if (
        left !in 0..MAX_COORDINATE ||
          top !in 0..MAX_COORDINATE ||
          width !in 1..MAX_EXTENT ||
          height !in 1..MAX_EXTENT
      ) {
        return null
      }
      return NativePlaybackViewport(left, top, width, height, fullscreen)
    }
  }
}

@ConsistentCopyVisibility
internal data class NativePlaybackControls private constructor(
  val volume: Float,
  val muted: Boolean,
  val paused: Boolean,
) {
  companion object {
    fun parse(volume: Float, muted: Boolean, paused: Boolean): NativePlaybackControls? {
      if (!volume.isFinite() || volume !in 0.0f..1.0f) {
        return null
      }
      return NativePlaybackControls(volume, muted, paused)
    }
  }
}

internal fun NativePlaybackControls.withForcedSilence(forceSilent: Boolean): NativePlaybackControls =
  if (forceSilent) {
    checkNotNull(NativePlaybackControls.parse(0.0f, true, paused))
  } else {
    this
  }

internal enum class NativePlaybackStartDecision {
  CREATE,
  UPDATE,
  REJECT,
}

internal fun nativePlaybackStartDecision(
  owned: NativePlaybackIdentity?,
  requested: NativePlaybackIdentity,
): NativePlaybackStartDecision = when {
  owned == null -> NativePlaybackStartDecision.CREATE
  owned == requested -> NativePlaybackStartDecision.UPDATE
  else -> NativePlaybackStartDecision.REJECT
}

/**
 * Main-thread lifecycle fence for presentation starts posted by JNI workers.
 *
 * A ticket is captured before a start is posted. Pausing invalidates every
 * outstanding ticket, and resuming does not revive one, so a runnable delayed
 * across even a quick pause/resume cycle still fails closed.
 */
internal class NativePlaybackLifecycleFence {
  private val lock = Any()
  private var revision = Any()
  private var suspended = true

  fun startTicket(): Ticket? = synchronized(lock) {
    if (suspended) null else Ticket(revision)
  }

  fun permits(ticket: Ticket): Boolean = synchronized(lock) {
    !suspended && ticket.revision === revision
  }

  fun suspend() = synchronized(lock) {
    suspended = true
    revision = Any()
  }

  fun resume() = synchronized(lock) {
    revision = Any()
    suspended = false
  }

  class Ticket internal constructor(internal val revision: Any)
}

/**
 * Coordinates one posted UI-thread operation with its bounded caller wait.
 *
 * Cancellation and execution claim the request under the same lock. If the
 * timeout wins before execution, the posted runnable becomes a no-op. If the
 * operation was already running, its result is rolled back on the UI thread
 * before the runnable returns. This keeps a timed-out native start from
 * publishing an unowned player.
 */
internal class NativePlaybackMainThreadRequest<T>(
  private val fallback: T,
) {
  private val lock = Any()
  private val completed = CountDownLatch(1)
  private var state = State.PENDING
  private var result = fallback

  fun execute(
    operation: () -> T,
    rollback: (T) -> Unit = {},
  ) {
    synchronized(lock) {
      if (state != State.PENDING) {
        completed.countDown()
        return
      }
      state = State.RUNNING
    }

    val produced = try {
      operation()
    } catch (_: RuntimeException) {
      fallback
    }
    val cancelled = synchronized(lock) {
      if (state == State.CANCELLED) {
        result = fallback
        state = State.COMPLETED
        true
      } else {
        result = produced
        state = State.COMPLETED
        false
      }
    }
    if (cancelled) {
      try {
        rollback(produced)
      } catch (_: RuntimeException) {
        // Rollback is best-effort and must not strand the waiting caller.
      }
    }
    completed.countDown()
  }

  fun await(timeout: Long, unit: TimeUnit): T {
    try {
      if (completed.await(timeout, unit)) {
        return synchronized(lock) { result }
      }
    } catch (_: InterruptedException) {
      Thread.currentThread().interrupt()
    }

    return synchronized(lock) {
      if (state == State.COMPLETED) {
        result
      } else {
        state = State.CANCELLED
        fallback
      }
    }
  }

  private enum class State {
    PENDING,
    RUNNING,
    CANCELLED,
    COMPLETED,
  }
}
