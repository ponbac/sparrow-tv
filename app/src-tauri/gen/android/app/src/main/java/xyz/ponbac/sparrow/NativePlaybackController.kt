package xyz.ponbac.sparrow

import android.graphics.Color
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSource
import androidx.media3.exoplayer.DecoderCounters
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.AspectRatioFrameLayout
import androidx.media3.ui.PlayerView
import java.util.Locale
import java.util.concurrent.TimeUnit
import kotlin.math.max

@OptIn(UnstableApi::class)
internal class NativePlaybackController(
  private val activity: MainActivity,
  private val forceSilent: Boolean,
) {
  private var host: PlaybackHost? = null
  private val lifecycleFence = NativePlaybackLifecycleFence()

  fun start(
    sessionId: String,
    streamHandle: String,
    left: Int,
    top: Int,
    width: Int,
    height: Int,
    volume: Float,
    muted: Boolean,
    fullscreen: Boolean,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return false
    val viewport =
      NativePlaybackViewport.parse(left, top, width, height, fullscreen) ?: return false
    val controls =
      NativePlaybackControls.parse(volume, muted, false)?.withForcedSilence(forceSilent)
        ?: return false
    val lifecycleTicket = lifecycleFence.startTicket() ?: return false
    return onMain(
      fallback = StartOutcome(false, null, false),
      rollback = { outcome ->
        val identityToRollback = outcome.boundIdentity
        val currentHost = host
        if (identityToRollback != null && currentHost != null) {
          currentHost.unbind(identityToRollback, revealWebContent = true)
          if (outcome.createdHost && currentHost.identity == null) {
            currentHost.release()
            if (host === currentHost) {
              host = null
            }
          }
        }
      },
    ) {
      if (!lifecycleFence.permits(lifecycleTicket)) {
        StartOutcome(false, null, false)
      } else {
        when (nativePlaybackStartDecision(host?.identity, identity)) {
          NativePlaybackStartDecision.UPDATE -> {
            val current = checkNotNull(host)
            current.setViewport(identity, viewport)
            current.setControls(identity, controls)
            StartOutcome(true, null, false)
          }
          NativePlaybackStartDecision.REJECT -> StartOutcome(false, null, false)
          NativePlaybackStartDecision.CREATE -> {
            val existingHost = host
            val current = existingHost ?: createPlaybackHost()
            if (current == null) {
              StartOutcome(false, null, false)
            } else {
              val createdHost = existingHost == null
              if (createdHost) {
                host = current
              }
              if (!current.bind(identity, viewport, controls)) {
                if (createdHost) {
                  current.release()
                  if (host === current) {
                    host = null
                  }
                }
                StartOutcome(false, null, false)
              } else {
                StartOutcome(true, identity, createdHost)
              }
            }
          }
        }
      }
    }.accepted
  }

  fun status(sessionId: String, streamHandle: String): String {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return ""
    return onMain("") {
      host?.status(identity) ?: ""
    }
  }

  fun setControls(
    sessionId: String,
    streamHandle: String,
    volume: Float,
    muted: Boolean,
    paused: Boolean,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return false
    val controls =
      NativePlaybackControls.parse(volume, muted, paused)?.withForcedSilence(forceSilent)
        ?: return false
    return onMain(false) {
      host?.setControls(identity, controls) ?: false
    }
  }

  fun setViewport(
    sessionId: String,
    streamHandle: String,
    left: Int,
    top: Int,
    width: Int,
    height: Int,
    fullscreen: Boolean,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return false
    val viewport =
      NativePlaybackViewport.parse(left, top, width, height, fullscreen) ?: return false
    return onMain(false) {
      host?.setViewport(identity, viewport) ?: false
    }
  }

  fun stop(sessionId: String, streamHandle: String): Boolean {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return false
    return onMain(false) {
      host?.unbind(identity, revealWebContent = false) ?: true
    }
  }

  fun stopSession(sessionId: String): Boolean {
    if (!NativePlaybackIdentity.isValidSessionId(sessionId)) {
      return false
    }
    return onMain(false) {
      host?.unbindSession(sessionId, revealWebContent = true) ?: true
    }
  }

  fun suspendSession(sessionId: String): Boolean {
    if (!NativePlaybackIdentity.isValidSessionId(sessionId)) {
      return false
    }
    return onMain(false) {
      host?.unbindSession(sessionId, revealWebContent = false) ?: true
    }
  }

  fun suspendAll(): Boolean = onMain(false) {
    host?.unbindAll(revealWebContent = false)
    true
  }

  fun stopAll(): Boolean = onMain(false) {
    host?.release()
    host = null
    true
  }

  fun pauseForLifecycle(): Boolean = onMain(false) {
    lifecycleFence.suspend()
    host?.pauseForLifecycle()
    true
  }

  fun resumeForLifecycle(): Boolean = onMain(false) {
    lifecycleFence.resume()
    true
  }

  private fun createPlaybackHost(): PlaybackHost? {
    val parent = activity.window.decorView as? ViewGroup ?: return null
    var playerView: PlayerView? = null
    var player: ExoPlayer? = null
    return try {
      val createdPlayerView = PlayerView(activity).apply {
        useController = false
        resizeMode = AspectRatioFrameLayout.RESIZE_MODE_FIT
        setShutterBackgroundColor(Color.BLACK)
        setShowBuffering(PlayerView.SHOW_BUFFERING_WHEN_PLAYING)
        setKeepContentOnPlayerReset(true)
        isClickable = false
        isFocusable = false
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
      }
      playerView = createdPlayerView
      val createdPlayer =
        ExoPlayer.Builder(activity)
          .setReleaseTimeoutMs(NATIVE_PLAYBACK_RELEASE_TIMEOUT_MS)
          .setDetachSurfaceTimeoutMs(NATIVE_PLAYBACK_RELEASE_TIMEOUT_MS)
          .build()
      player = createdPlayer
      createdPlayerView.player = createdPlayer
      parent.addView(createdPlayerView)
      PlaybackHost(activity, parent, createdPlayerView, createdPlayer)
    } catch (_: RuntimeException) {
      runCatching { playerView?.player = null }
      runCatching { player?.release() }
      runCatching { playerView?.let(parent::removeView) }
      null
    }
  }

  private fun <T> onMain(
    fallback: T,
    rollback: (T) -> Unit = {},
    operation: () -> T,
  ): T {
    if (Looper.myLooper() == Looper.getMainLooper()) {
      return try {
        operation()
      } catch (_: RuntimeException) {
        fallback
      }
    }

    val request = NativePlaybackMainThreadRequest(fallback)
    return try {
      activity.runOnUiThread {
        request.execute(operation, rollback)
      }
      request.await(2, TimeUnit.SECONDS)
    } catch (_: RuntimeException) {
      request.await(0, TimeUnit.NANOSECONDS)
    }
  }

  private data class StartOutcome(
    val accepted: Boolean,
    val boundIdentity: NativePlaybackIdentity?,
    val createdHost: Boolean,
  )

  private class PlaybackHost(
    private val activity: MainActivity,
    private val parent: ViewGroup,
    private val playerView: PlayerView,
    private val player: ExoPlayer,
  ) {
    private var binding: PlaybackBinding? = null
    private var released = false

    val identity: NativePlaybackIdentity?
      get() = binding?.identity

    fun bind(
      identity: NativePlaybackIdentity,
      viewport: NativePlaybackViewport,
      controls: NativePlaybackControls,
    ): Boolean {
      if (released || binding != null) {
        return false
      }
      val created = PlaybackBinding(identity, player, controls)
      binding = created
      return try {
        player.addListener(created)
        player.addAnalyticsListener(created)
        val dataSourceFactory = DataSource.Factory { NativePlaybackDataSource(identity) }
        val mediaSource =
          DefaultMediaSourceFactory(activity)
            .setDataSourceFactory(dataSourceFactory)
            .createMediaSource(
              MediaItem.Builder()
                .setUri(NativePlaybackDataSource.PLAYBACK_URI)
                .setMimeType(MimeTypes.VIDEO_MP2T)
                .build(),
            )
        player.setMediaSource(mediaSource)
        setViewport(viewport)
        created.applyInitialControls()
        player.prepare()
        true
      } catch (_: RuntimeException) {
        if (binding === created) {
          binding = null
        }
        created.release()
        resetPlayer()
        revealWebContent()
        false
      }
    }

    fun status(identity: NativePlaybackIdentity): String =
      binding?.takeIf { it.identity == identity }?.statusJson() ?: ""

    fun setControls(
      identity: NativePlaybackIdentity,
      controls: NativePlaybackControls,
    ): Boolean {
      val current = binding?.takeIf { it.identity == identity } ?: return false
      current.setControls(controls)
      return true
    }

    fun setViewport(
      identity: NativePlaybackIdentity,
      viewport: NativePlaybackViewport,
    ): Boolean {
      if (binding?.identity != identity) {
        return false
      }
      setViewport(viewport)
      return true
    }

    fun unbind(
      identity: NativePlaybackIdentity,
      revealWebContent: Boolean,
    ): Boolean {
      val current = binding ?: return true
      if (current.identity != identity) {
        return false
      }
      binding = null
      current.release()
      resetPlayer()
      if (revealWebContent) {
        revealWebContent()
      }
      return true
    }

    fun unbindSession(
      sessionId: String,
      revealWebContent: Boolean,
    ): Boolean {
      val current = binding ?: return true
      if (current.identity.sessionId != sessionId) {
        return false
      }
      return unbind(current.identity, revealWebContent)
    }

    fun unbindAll(revealWebContent: Boolean) {
      val current = binding ?: return
      unbind(current.identity, revealWebContent)
    }

    fun pauseForLifecycle() {
      if (released) {
        return
      }
      binding?.pauseForLifecycle()
    }

    fun revealWebContent() {
      if (!released) {
        directWebContentChild(parent)?.bringToFront()
      }
    }

    fun release() {
      if (released) {
        return
      }
      binding?.release()
      binding = null
      resetPlayer()
      released = true
      runCatching { playerView.player = null }
      runCatching { player.release() }
      runCatching { parent.removeView(playerView) }
    }

    private fun setViewport(viewport: NativePlaybackViewport) {
      if (released) {
        return
      }
      val webView = findWebView(parent) ?: return
      val webLocation = IntArray(2)
      val parentLocation = IntArray(2)
      webView.getLocationInWindow(webLocation)
      parent.getLocationInWindow(parentLocation)
      playerView.layoutParams =
        FrameLayout.LayoutParams(viewport.width, viewport.height).apply {
          leftMargin = webLocation[0] - parentLocation[0] + viewport.left
          topMargin = webLocation[1] - parentLocation[1] + viewport.top
        }
      playerView.bringToFront()
    }

    private fun resetPlayer() {
      runCatching { player.stop() }
      runCatching { player.clearMediaItems() }
    }

    companion object {
      private fun directWebContentChild(parent: ViewGroup): View? {
        var content: View = findWebView(parent) ?: return null
        while (true) {
          val ancestor = content.parent
          if (ancestor === parent) {
            return content
          }
          content = ancestor as? View ?: return null
        }
      }
    }
  }

  private class PlaybackBinding(
    val identity: NativePlaybackIdentity,
    private val player: ExoPlayer,
    initialControls: NativePlaybackControls,
  ) : Player.Listener, AnalyticsListener {
    private var phase = if (initialControls.paused) Phase.PAUSED else Phase.STARTING
    private var controls = initialControls
    private val renderedFrames = NativePlaybackFrameCounter()
    private var droppedFrames = 0L
    private var released = false

    fun applyInitialControls() {
      if (released) {
        return
      }
      player.volume = if (controls.muted) 0.0f else controls.volume
      player.playWhenReady = !controls.paused
    }

    fun setControls(next: NativePlaybackControls) {
      if (released) {
        return
      }
      controls = next
      player.volume = if (next.muted) 0.0f else next.volume
      if (next.paused) {
        player.pause()
        phase = Phase.PAUSED
      } else {
        player.play()
        if (!player.isPlaying) {
          phase = Phase.STARTING
        }
      }
    }

    fun pauseForLifecycle() {
      if (released) {
        return
      }
      player.pause()
      phase = Phase.PAUSED
    }

    fun statusJson(): String {
      val bufferedDuration =
        if (
          player.currentPosition == C.TIME_UNSET ||
            player.bufferedPosition == C.TIME_UNSET
        ) {
          0L
        } else {
          max(0L, player.bufferedPosition - player.currentPosition)
        }
      return String.format(
        Locale.ROOT,
        "{\"state\":\"%s\",\"decodedFrames\":%d,\"droppedFrames\":%d,\"bufferedDurationMs\":%d,\"silent\":%s}",
        phase.wire,
        renderedFrames.value().coerceAtMost(MAX_SAFE_COUNTER),
        droppedFrames.coerceAtMost(MAX_SAFE_COUNTER),
        bufferedDuration.coerceAtMost(MAX_SAFE_COUNTER),
        controls.muted || controls.volume == 0.0f,
      )
    }

    fun release() {
      if (released) {
        return
      }
      released = true
      phase = Phase.STOPPED
      runCatching { player.removeListener(this) }
      runCatching { player.removeAnalyticsListener(this) }
    }

    override fun onPlaybackStateChanged(playbackState: Int) {
      if (released || controls.paused) {
        return
      }
      phase =
        when (playbackState) {
          Player.STATE_ENDED -> Phase.FAILED
          Player.STATE_READY -> if (player.isPlaying) Phase.PLAYING else Phase.STARTING
          Player.STATE_IDLE, Player.STATE_BUFFERING -> Phase.STARTING
          else -> Phase.FAILED
        }
    }

    override fun onIsPlayingChanged(isPlaying: Boolean) {
      if (released) {
        return
      }
      phase =
        when {
          controls.paused -> Phase.PAUSED
          isPlaying -> Phase.PLAYING
          phase == Phase.FAILED -> Phase.FAILED
          else -> Phase.STARTING
        }
    }

    override fun onPlayerError(error: PlaybackException) {
      if (!released) {
        phase = Phase.FAILED
      }
    }

    override fun onDroppedVideoFrames(
      eventTime: AnalyticsListener.EventTime,
      droppedFrames: Int,
      elapsedMs: Long,
    ) {
      if (!released && droppedFrames > 0) {
        this.droppedFrames =
          (this.droppedFrames + droppedFrames.toLong()).coerceAtMost(MAX_SAFE_COUNTER)
      }
    }

    override fun onVideoEnabled(
      eventTime: AnalyticsListener.EventTime,
      decoderCounters: DecoderCounters,
    ) {
      if (!released) {
        renderedFrames.onVideoEnabled(decoderCounters)
      }
    }

    override fun onVideoDisabled(
      eventTime: AnalyticsListener.EventTime,
      decoderCounters: DecoderCounters,
    ) {
      if (!released) {
        renderedFrames.onVideoDisabled(decoderCounters)
      }
    }

    override fun onVideoFrameProcessingOffset(
      eventTime: AnalyticsListener.EventTime,
      totalProcessingOffsetUs: Long,
      frameCount: Int,
    ) {
      if (!released && frameCount > 0) {
        renderedFrames.onVideoFrameProcessingOffset(frameCount)
      }
    }

    private enum class Phase(val wire: String) {
      STARTING("starting"),
      PLAYING("playing"),
      PAUSED("paused"),
      FAILED("failed"),
      STOPPED("stopped"),
    }

    companion object {
      private const val MAX_SAFE_COUNTER = 9_007_199_254_740_991L
    }
  }

  companion object {
    private fun findWebView(view: View): WebView? {
      if (view is WebView) {
        return view
      }
      if (view is ViewGroup) {
        for (index in 0 until view.childCount) {
          val found = findWebView(view.getChildAt(index))
          if (found != null) {
            return found
          }
        }
      }
      return null
    }
  }
}

internal class NativePlaybackFrameCounter {
  private var completedDecoderFrames = 0L
  private var processingOffsetFrames = 0L
  private var activeDecoderCounters: DecoderCounters? = null

  fun onVideoEnabled(decoderCounters: DecoderCounters) {
    if (activeDecoderCounters === decoderCounters) {
      return
    }
    completeActiveDecoder()
    activeDecoderCounters = decoderCounters
  }

  fun onVideoDisabled(decoderCounters: DecoderCounters) {
    if (activeDecoderCounters !== decoderCounters) {
      return
    }
    completeActiveDecoder()
  }

  fun onVideoFrameProcessingOffset(frameCount: Int) {
    if (frameCount > 0) {
      processingOffsetFrames = safeFrameSum(processingOffsetFrames, frameCount.toLong())
    }
  }

  fun value(): Long {
    val activeFrames = activeDecoderCounters?.renderedOutputBufferCount?.toLong() ?: 0L
    val decoderFrames = safeFrameSum(completedDecoderFrames, activeFrames.coerceAtLeast(0L))
    return max(decoderFrames, processingOffsetFrames)
  }

  private fun completeActiveDecoder() {
    val active = activeDecoderCounters ?: return
    completedDecoderFrames =
      safeFrameSum(completedDecoderFrames, active.renderedOutputBufferCount.toLong().coerceAtLeast(0L))
    activeDecoderCounters = null
  }

  private fun safeFrameSum(left: Long, right: Long): Long =
    (left + right).coerceAtMost(MAX_SAFE_COUNTER)

  private companion object {
    const val MAX_SAFE_COUNTER = 9_007_199_254_740_991L
  }
}
