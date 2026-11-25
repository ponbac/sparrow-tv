package xyz.ponbac.sparrow.shield

import android.app.Activity
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.SurfaceView
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.annotation.Keep
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.ui.PlayerView
import xyz.ponbac.sparrow.NativePlaybackController
import xyz.ponbac.sparrow.NativePlaybackControls
import xyz.ponbac.sparrow.NativePlaybackIdentity
import xyz.ponbac.sparrow.NativePlaybackLifecycleFence
import xyz.ponbac.sparrow.NativePlaybackPlacement
import xyz.ponbac.sparrow.NativePlaybackReader
import xyz.ponbac.sparrow.NativePlaybackSignal
import xyz.ponbac.sparrow.NativePlaybackViewport
import xyz.ponbac.sparrow.PlaybackAudioFocus

internal const val PICTURE_REVEAL_MS = 240L

/** Per-Activity Media3 adapter. JNI submits work and gets its completion as an event. */
@Keep
class ShieldPlayback(
  private val activity: Activity,
  root: FrameLayout,
  guide: SurfaceView,
  private val hostId: Long,
) {
  private val main = Handler(Looper.getMainLooper())
  private val lifecycle = NativePlaybackLifecycleFence()
  private val placement = ShieldPlaybackPlacement(root, guide)
  private val controller = NativePlaybackController(
    activity, placement, forceSilent = false,
    lifecycleFence = lifecycle,
  )

  @Volatile private var destroyed = false
  private var latestGeneration = 0L
  private var binding: Binding? = null
  private var retained: Binding? = null
  private var wakeGeneration = 0L
  private var wakeRequest = 0L

  // Read on Media3's loader thread. Rust's reader also checks the full identity.
  @Volatile private var loaderGeneration = 0L

  fun onActivityStarted() {
    controller.resumeForLifecycle()
  }

  fun onActivityStopped() {
    controller.pauseForLifecycle()
    binding?.let { current ->
      current.visibilityPaused = true
      current.focus.release()
    }
    activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
  }

  fun onActivityDestroyed() {
    destroyed = true
    main.removeCallbacksAndMessages(null)
    onActivityStopped()
    loaderGeneration = 0L
    binding = null
    retained = null
    controller.stopAll()
  }

  fun startPlayback(
    host: Long,
    generation: Long,
    request: Long,
    session: String,
    stream: String,
    left: Int,
    top: Int,
    width: Int,
    height: Int,
    volume: Float,
    muted: Boolean,
    paused: Boolean,
    fullscreen: Boolean,
  ): Boolean {
    val ticket = lifecycle.startTicket() ?: return false
    val identity = NativePlaybackIdentity.parse(session, stream) ?: return false
    val viewport = NativePlaybackViewport.parse(left, top, width, height, fullscreen) ?: return false
    val controls = NativePlaybackControls.parse(volume, muted, paused) ?: return false
    return post(host, generation, request, Operation.START) {
      if (lifecycle.permits(ticket) && generation > latestGeneration && binding == null) {
        start(Presentation(generation, identity), viewport, controls).result()
      } else {
        Result.REJECTED
      }
    }
  }

  fun setPlaybackViewport(
    host: Long,
    generation: Long,
    request: Long,
    session: String,
    stream: String,
    left: Int,
    top: Int,
    width: Int,
    height: Int,
    fullscreen: Boolean,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(session, stream) ?: return false
    val viewport = if (width == 0 && height == 0) {
      null
    } else {
      NativePlaybackViewport.parse(left, top, width, height, fullscreen) ?: return false
    }
    val presentation = Presentation(generation, identity)
    return post(host, generation, request, Operation.VIEWPORT) {
      val live = binding
      val current = live ?: retained
      if (current?.presentation != presentation || request <= current.viewportRequest) {
        Result.REJECTED
      } else {
        current.viewportRequest = request
        when {
          viewport == null -> {
            placement.hideUntilPlaced()
            Result.ACCEPTED
          }
          live == null -> controller.setRetiredViewport(viewport).result()
          else -> controller.setViewport(identity, viewport).result()
        }
      }
    }
  }

  fun setPlaybackControls(
    host: Long,
    generation: Long,
    request: Long,
    session: String,
    stream: String,
    volume: Float,
    muted: Boolean,
    paused: Boolean,
  ): Boolean {
    val controls = NativePlaybackControls.parse(volume, muted, paused) ?: return false
    return postForBinding(host, generation, request, Operation.CONTROLS, session, stream) { current ->
      if (request <= current.controlsRequest) {
        Result.REJECTED
      } else {
        current.controlsRequest = request
        // Accept controls for the retiring stream, including across a quick
        // return to the Activity. Rust decides whether to present a new stream.
        val effective = if (current.visibilityPaused && !controls.paused) {
          checkNotNull(NativePlaybackControls.parse(volume, muted, paused = true))
        } else {
          controls
        }
        val applied = controller.setControls(current.presentation.identity, effective)
        if (applied) {
          current.focus.setPlaying(!effective.paused)
        }
        applied.result()
      }
    }
  }

  fun samplePlayback(
    host: Long,
    generation: Long,
    request: Long,
    session: String,
    stream: String,
  ): Boolean = postForBinding(host, generation, request, Operation.SAMPLE, session, stream) { current ->
    val status = controller.status(current.presentation.identity)
    Result(status.isNotEmpty(), status)
  }

  fun stopPlayback(
    host: Long,
    generation: Long,
    request: Long,
    session: String,
    stream: String,
    retainPicture: Boolean,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(session, stream) ?: return false
    return post(host, generation, request, Operation.STOP) {
      stop(Presentation(generation, identity), retainPicture).result()
    }
  }

  fun setPlaybackScreenWake(host: Long, generation: Long, request: Long, enabled: Boolean): Boolean {
    val ticket = if (enabled) lifecycle.startTicket() ?: return false else null
    return post(host, generation, request, Operation.SCREEN_WAKE) {
      if (generation < wakeGeneration || (generation == wakeGeneration && request <= wakeRequest) ||
          (ticket != null && !lifecycle.permits(ticket))) {
        Result.REJECTED
      } else {
        wakeGeneration = generation
        wakeRequest = request
        val flag = WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        if (enabled) activity.window.addFlags(flag) else activity.window.clearFlags(flag)
        Result.ACCEPTED
      }
    }
  }

  private fun start(
    presentation: Presentation,
    viewport: NativePlaybackViewport,
    controls: NativePlaybackControls,
  ): Boolean {
    val (generation, identity) = presentation
    latestGeneration = generation
    val focus = PlaybackAudioFocus(activity) { signal(presentation, NativePlaybackSignal.AUDIO_FOCUS_LOST) }
    val reader = NativePlaybackReader { source, output, offset, length ->
      if (loaderGeneration == generation) {
        readNativePlayback(hostId, generation, source.sessionId, source.streamHandle, output, offset, length)
      } else {
        -1
      }
    }
    loaderGeneration = generation
    if (!controller.start(identity, viewport, controls, reader) { signal(presentation, it) }) {
      loaderGeneration = 0L
      return false
    }
    retained = null
    binding = Binding(presentation, focus)
    focus.setPlaying(!controls.paused)
    return true
  }

  private fun stop(presentation: Presentation, retainPicture: Boolean): Boolean {
    val current = binding
    if (current?.presentation?.generation == presentation.generation) {
      if (current.presentation != presentation) return false
      loaderGeneration = 0L
      if (!controller.stop(presentation.identity, revealContent = !retainPicture)) return false
      current.focus.release()
      binding = null
      retained = current.takeIf { retainPicture }
    } else if (current == null && !retainPicture) {
      val previous = retained?.presentation
      // A replacement may have failed before it bound the player. Its clear
      // retires the old picture unless a more recent generation owns the gate.
      if (previous != null && (previous == presentation ||
          (previous.generation < presentation.generation && presentation.generation >= latestGeneration))) {
        controller.revealRetiredPicture()
        retained = null
      }
    }
    // Retire even an unbound generation. A timed-out start arriving after
    // cleanup cannot claim the player later.
    latestGeneration = maxOf(latestGeneration, presentation.generation)
    return true
  }

  private fun signal(presentation: Presentation, signal: NativePlaybackSignal) {
    val ticket = lifecycle.startTicket() ?: return
    main.post {
      val current = binding
      if (!destroyed && lifecycle.permits(ticket) && current?.presentation == presentation && !current.visibilityPaused) {
        nativePlaybackSignal(hostId, presentation.generation, signal.wire)
      }
    }
  }

  private fun postForBinding(
    host: Long,
    generation: Long,
    request: Long,
    operation: Operation,
    session: String,
    stream: String,
    perform: (Binding) -> Result,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(session, stream) ?: return false
    val presentation = Presentation(generation, identity)
    return post(host, generation, request, operation) {
      val current = binding
      // The owner can have an in-flight request while processing HostVisibility.
      // Stop retires the exact binding after the transport is suspended.
      if (current?.presentation == presentation) {
        perform(current)
      } else {
        Result.REJECTED
      }
    }
  }

  private fun post(
    host: Long,
    generation: Long,
    request: Long,
    operation: Operation,
    perform: () -> Result,
  ): Boolean {
    if (host != hostId || generation <= 0 || request <= 0 || destroyed) return false
    return main.post {
      if (!destroyed) {
        val result = try {
          perform()
        } catch (_: RuntimeException) {
          Result.REJECTED
        }
        nativePlaybackAck(hostId, generation, request, operation.wire, result.accepted, result.status)
      }
    }
  }

  private fun Boolean.result(): Result = if (this) Result.ACCEPTED else Result.REJECTED

  private data class Presentation(val generation: Long, val identity: NativePlaybackIdentity)

  private class Binding(
    val presentation: Presentation,
    val focus: PlaybackAudioFocus,
  ) {
    var controlsRequest = 0L
    var viewportRequest = 0L
    var visibilityPaused = false
  }

  private enum class Operation(val wire: Int) {
    START(1), VIEWPORT(2), CONTROLS(3), SAMPLE(4), STOP(5), SCREEN_WAKE(6),
  }

  private data class Result(val accepted: Boolean, val status: String = "") {
    companion object {
      val ACCEPTED = Result(true)
      val REJECTED = Result(false)
    }
  }

  private external fun nativePlaybackAck(
    host: Long, generation: Long, request: Long, operation: Int, accepted: Boolean, status: String,
  )

  private external fun nativePlaybackSignal(host: Long, generation: Long, signal: Int)

  private external fun readNativePlayback(
    host: Long,
    generation: Long,
    session: String,
    stream: String,
    output: ByteArray,
    offset: Int,
    length: Int,
  ): Int
}

/** The picture surface stays above GPUI's SurfaceView and below the Android editor. */
@OptIn(UnstableApi::class)
private class ShieldPlaybackPlacement(
  private val root: FrameLayout,
  private val guide: SurfaceView,
) : NativePlaybackPlacement {
  private var mountedPlayer: PlayerView? = null

  // Rust supplies the explicit bounds in both modes. Media3 retains its view.
  override val televisionFullscreen: Boolean? = null
  override fun container(): ViewGroup = root

  override fun createPlayerView(activity: Activity): PlayerView = PlayerView(activity).apply {
    (videoSurfaceView as SurfaceView).setZOrderMediaOverlay(true)
    visibility = View.INVISIBLE
  }

  override fun mount(parent: ViewGroup, playerView: PlayerView) {
    parent.addView(playerView, parent.indexOfChild(guide) + 1, FrameLayout.LayoutParams(1, 1))
    mountedPlayer = playerView
  }

  fun hideUntilPlaced() {
    // Keep the SurfaceView attached so a retained frame can return after layout.
    val player = mountedPlayer ?: return
    val params = player.layoutParams as FrameLayout.LayoutParams
    val right = maxOf(player.right, params.leftMargin + params.width)
    player.translationX = -(right + root.width).toFloat()
  }

  override fun place(parent: ViewGroup, playerView: PlayerView, viewport: NativePlaybackViewport) {
    val x = guide.left + viewport.left
    val y = guide.top + viewport.top
    val current = playerView.layoutParams as FrameLayout.LayoutParams
    if (current.leftMargin != x || current.topMargin != y ||
        current.width != viewport.width || current.height != viewport.height) {
      playerView.layoutParams = FrameLayout.LayoutParams(viewport.width, viewport.height, Gravity.TOP or Gravity.LEFT).apply {
        leftMargin = x
        topMargin = y
      }
    }
    playerView.translationX = 0f
    playerView.visibility = View.VISIBLE
  }

  override fun fillWindow(parent: ViewGroup, playerView: PlayerView) = Unit // TV has no PiP.

  override fun unmount(parent: ViewGroup, playerView: PlayerView) {
    if (mountedPlayer === playerView) mountedPlayer = null
    parent.removeView(playerView)
  }

  override fun revealContent(parent: ViewGroup, playerView: PlayerView) {
    playerView.visibility = View.INVISIBLE
  }
}
