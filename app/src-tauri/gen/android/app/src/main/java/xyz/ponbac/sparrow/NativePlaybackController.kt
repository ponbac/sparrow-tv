package xyz.ponbac.sparrow

import android.graphics.Color
import android.graphics.Rect
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
import androidx.media3.common.Tracks
import androidx.media3.common.VideoSize
import androidx.media3.common.util.UnstableApi
import androidx.media3.common.util.Util
import androidx.media3.datasource.DataSource
import androidx.media3.exoplayer.DecoderCounters
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.audio.AudioSink
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
  private var pictureInPicture = false
  private var pictureInPictureCover: View? = null
  private var pictureListener: (() -> Unit)? = null
  private var videoAspect = pictureInPictureAspect(0, 0, 1f)

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

  /** Rust's lifecycle suspension: everything goes while the app is stopped. */
  fun suspendAll(): Boolean = onMain(false) {
    host?.suspendForLifecycle()
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

  /** Called on the main thread when the picture's size or place changes. */
  fun setPictureListener(listener: (() -> Unit)?) {
    pictureListener = listener
  }

  /** The playing video's shape for a picture-in-picture window. */
  fun videoAspect(): Pair<Int, Int> = videoAspect

  /**
   * Where the picture is on screen, for the window to grow out of. Inside
   * picture-in-picture the picture is the window, and its bounds say nothing.
   */
  fun pictureBounds(): Rect? = if (pictureInPicture) null else host?.pictureBounds()

  /** A paused surface cannot move with a TV entering or leaving fullscreen. */
  fun revealRetiredPicture() {
    host?.takeIf { it.identity == null }?.revealWebContent()
  }

  /**
   * The picture-in-picture window shows the whole Activity, so the picture
   * fills it over an opaque cover for as long as it lasts: the page below
   * never shows in it, whether or not a picture is bound. Leaving puts the
   * picture back where the page last placed it, or shows the page when
   * nothing is playing.
   */
  fun setPictureInPicture(active: Boolean) {
    if (pictureInPicture == active) {
      return
    }
    pictureInPicture = active
    val parent = activity.window.decorView as? ViewGroup ?: return
    if (active) {
      val cover = View(activity).apply {
        setBackgroundColor(Color.BLACK)
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
      }
      runCatching {
        parent.addView(
          cover,
          FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
          ),
        )
        pictureInPictureCover = cover
      }
      host?.setPictureInPicture(true)
    } else {
      host?.setPictureInPicture(false)
      pictureInPictureCover?.let { cover -> runCatching { parent.removeView(cover) } }
      pictureInPictureCover = null
    }
  }

  private fun onVideoSize(videoSize: VideoSize) {
    // A reset player reports an unknown size: keep the last known shape.
    if (videoSize.width > 0 && videoSize.height > 0) {
      videoAspect = pictureInPictureAspect(
        videoSize.width,
        videoSize.height,
        videoSize.pixelWidthHeightRatio,
      )
      pictureListener?.invoke()
    }
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
        descendantFocusability = ViewGroup.FOCUS_BLOCK_DESCENDANTS
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
      }
      playerView = createdPlayerView
      val createdPlayer =
        ExoPlayer.Builder(activity, NativePlaybackRenderersFactory(activity))
          .setReleaseTimeoutMs(NATIVE_PLAYBACK_RELEASE_TIMEOUT_MS)
          .setDetachSurfaceTimeoutMs(NATIVE_PLAYBACK_RELEASE_TIMEOUT_MS)
          .build()
      player = createdPlayer
      createdPlayerView.player = createdPlayer
      createdPlayer.addListener(
        object : Player.Listener {
          override fun onVideoSizeChanged(videoSize: VideoSize) = onVideoSize(videoSize)
        },
      )
      createdPlayerView.addOnLayoutChangeListener { _, left, top, right, bottom,
          oldLeft, oldTop, oldRight, oldBottom ->
        if (left != oldLeft || top != oldTop || right != oldRight || bottom != oldBottom) {
          pictureListener?.invoke()
        }
      }
      parent.addView(createdPlayerView)
      PlaybackHost(activity, parent, createdPlayerView, createdPlayer).also {
        it.setPictureInPicture(pictureInPicture)
      }
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
    /**
     * The presentation the lifecycle took away, as the page last saw it. The
     * page hears of the suspension by an event that can arrive after its
     * next status read; until it suspends or stops the presentation itself,
     * the presentation reads as paused rather than missing, which would start
     * a recovery.
     */
    private var lifecycleSuspended: Pair<NativePlaybackIdentity, String>? = null
    private var pictureInPicture = false
    /** The page's last placement outside picture-in-picture. */
    private var placement: NativePlaybackViewport? = null

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
      lifecycleSuspended = null
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
      binding?.takeIf { it.identity == identity }?.statusJson()
        ?: lifecycleSuspended?.takeIf { it.first == identity }?.second
        ?: ""

    fun suspendForLifecycle() {
      val current = binding ?: return
      current.pauseForLifecycle()
      val status = current.statusJson()
      unbind(current.identity, revealWebContent = false)
      lifecycleSuspended = current.identity to status
    }

    private fun isLifecycleSuspended(identity: NativePlaybackIdentity): Boolean =
      lifecycleSuspended?.first == identity

    fun setControls(
      identity: NativePlaybackIdentity,
      controls: NativePlaybackControls,
    ): Boolean {
      val current = binding?.takeIf { it.identity == identity }
        ?: return isLifecycleSuspended(identity)
      current.setControls(controls)
      return true
    }

    fun setViewport(
      identity: NativePlaybackIdentity,
      viewport: NativePlaybackViewport,
    ): Boolean {
      if (binding?.identity != identity) {
        return isLifecycleSuspended(identity)
      }
      setViewport(viewport)
      return true
    }

    fun unbind(
      identity: NativePlaybackIdentity,
      revealWebContent: Boolean,
    ): Boolean {
      if (isLifecycleSuspended(identity)) {
        lifecycleSuspended = null
      }
      val current = binding ?: return true
      if (current.identity != identity) {
        return false
      }
      binding = null
      current.release()
      resetPlayer()
      // Fullscreen may have changed before the presentation finished retiring.
      val fullscreen = activity.televisionFullscreen
      if (revealWebContent || (fullscreen != null && fullscreen != placement?.fullscreen)) {
        revealWebContent()
      }
      return true
    }

    fun unbindSession(
      sessionId: String,
      revealWebContent: Boolean,
    ): Boolean {
      if (lifecycleSuspended?.first?.sessionId == sessionId) {
        lifecycleSuspended = null
      }
      val current = binding ?: return true
      if (current.identity.sessionId != sessionId) {
        return false
      }
      return unbind(current.identity, revealWebContent)
    }

    fun pauseForLifecycle() {
      if (released) {
        return
      }
      binding?.pauseForLifecycle()
    }

    fun revealWebContent() {
      // Picture-in-picture shows only the picture; leaving it decides.
      if (!released && !pictureInPicture) {
        directWebContentChild(parent)?.bringToFront()
      }
    }

    fun setPictureInPicture(active: Boolean) {
      if (released || pictureInPicture == active) {
        return
      }
      pictureInPicture = active
      if (active) {
        fillWindow()
        return
      }
      // A placement made inside picture-in-picture was for the small window.
      val last = placement
      if (binding != null && last != null) {
        place(last)
      } else {
        revealWebContent()
      }
    }

    fun pictureBounds(): Rect? {
      if (released || playerView.width == 0 || playerView.height == 0) {
        return null
      }
      val bounds = Rect()
      return if (playerView.getGlobalVisibleRect(bounds)) bounds else null
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
      if (pictureInPicture) {
        fillWindow()
        return
      }
      placement = viewport
      place(viewport)
    }

    private fun fillWindow() {
      playerView.layoutParams = FrameLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.MATCH_PARENT,
      )
      playerView.bringToFront()
    }

    private fun place(viewport: NativePlaybackViewport) {
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
    private var audioTracks = NativePlaybackAudioTracks.UNKNOWN
    private var audioDecoder: String? = null
    private var audioPassthrough = false
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
        "{\"state\":\"%s\",\"decodedFrames\":%d,\"droppedFrames\":%d,\"bufferedDurationMs\":%d,\"silent\":%s,\"audio\":\"%s\"}",
        phase.wire,
        renderedFrames.value().coerceAtMost(MAX_SAFE_COUNTER),
        droppedFrames.coerceAtMost(MAX_SAFE_COUNTER),
        bufferedDuration.coerceAtMost(MAX_SAFE_COUNTER),
        controls.muted || controls.volume == 0.0f,
        nativePlaybackAudio(audioTracks, audioDecoder, audioPassthrough).wire,
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

    override fun onTracksChanged(tracks: Tracks) {
      if (released) {
        return
      }
      val audio = tracks.groups.filter { it.type == C.TRACK_TYPE_AUDIO }
      audioTracks =
        when {
          tracks.isEmpty -> NativePlaybackAudioTracks.UNKNOWN
          audio.isEmpty() -> NativePlaybackAudioTracks.ABSENT
          // Media3 leaves a track unselected when no renderer can decode it.
          audio.none { it.isSelected } -> NativePlaybackAudioTracks.UNDECODABLE
          else -> NativePlaybackAudioTracks.SELECTED
        }
    }

    override fun onAudioDecoderInitialized(
      eventTime: AnalyticsListener.EventTime,
      decoderName: String,
      initializedTimestampMs: Long,
      initializationDurationMs: Long,
    ) {
      if (!released) {
        audioDecoder = decoderName
      }
    }

    override fun onAudioDecoderReleased(
      eventTime: AnalyticsListener.EventTime,
      decoderName: String,
    ) {
      if (!released && audioDecoder == decoderName) {
        audioDecoder = null
      }
    }

    override fun onAudioTrackInitialized(
      eventTime: AnalyticsListener.EventTime,
      audioTrackConfig: AudioSink.AudioTrackConfig,
    ) {
      if (!released) {
        // An output opened for encoded audio is given the track undecoded.
        audioPassthrough = !Util.isEncodingLinearPcm(audioTrackConfig.encoding)
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
