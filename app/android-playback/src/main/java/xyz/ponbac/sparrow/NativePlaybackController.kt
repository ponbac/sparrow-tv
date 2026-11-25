package xyz.ponbac.sparrow

import android.app.Activity
import android.graphics.Color
import android.graphics.Rect
import android.os.Looper
import android.view.View
import android.view.ViewGroup
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
import java.util.concurrent.TimeUnit
import kotlin.math.max

/** Main-thread placement for an attached PlayerView. Viewports are in host content pixels. */
@OptIn(UnstableApi::class)
internal interface NativePlaybackPlacement {
  val televisionFullscreen: Boolean?
  fun container(): ViewGroup?
  fun createPlayerView(activity: Activity): PlayerView = PlayerView(activity)
  fun mount(parent: ViewGroup, playerView: PlayerView)
  fun unmount(parent: ViewGroup, playerView: PlayerView) = parent.removeView(playerView)
  fun place(parent: ViewGroup, playerView: PlayerView, viewport: NativePlaybackViewport)
  fun fillWindow(parent: ViewGroup, playerView: PlayerView)
  fun revealContent(parent: ViewGroup, playerView: PlayerView)
}

internal enum class NativePlaybackSignal(val wire: Int) {
  FIRST_FRAME(1),
  NATURAL_END(2),
  STREAM_INTERRUPTED(3),
  PLAYER_ERROR(4),
  AUDIO_FOCUS_LOST(5),
}

@OptIn(UnstableApi::class)
internal class NativePlaybackController(
  private val activity: Activity,
  private val placement: NativePlaybackPlacement,
  private val forceSilent: Boolean,
  private val lifecycleFence: NativePlaybackLifecycleFence = NativePlaybackLifecycleFence(),
) {
  private var host: PlaybackHost? = null
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
    val controls = NativePlaybackControls.parse(volume, muted, false) ?: return false
    return start(identity, viewport, controls)
  }

  fun start(
    identity: NativePlaybackIdentity,
    viewport: NativePlaybackViewport,
    controls: NativePlaybackControls,
    reader: NativePlaybackReader? = null,
    observer: ((NativePlaybackSignal) -> Unit)? = null,
  ): Boolean {
    val initial = controls.withForcedSilence(forceSilent)
    val lifecycleTicket = lifecycleFence.startTicket() ?: return false
    return onMain(
      fallback = StartOutcome(false, null, false),
      rollback = { outcome ->
        val identityToRollback = outcome.boundIdentity
        val currentHost = host
        if (identityToRollback != null && currentHost != null) {
          currentHost.unbind(identityToRollback, revealContent = true)
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
            current.setControls(identity, initial)
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
              if (!current.bind(identity, viewport, initial, reader, observer)) {
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
    return status(identity)
  }

  fun status(identity: NativePlaybackIdentity): String = onMain("") {
    host?.status(identity) ?: ""
  }

  fun setControls(
    sessionId: String,
    streamHandle: String,
    volume: Float,
    muted: Boolean,
    paused: Boolean,
  ): Boolean {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return false
    val controls = NativePlaybackControls.parse(volume, muted, paused) ?: return false
    return setControls(identity, controls)
  }

  fun setControls(identity: NativePlaybackIdentity, controls: NativePlaybackControls): Boolean =
    onMain(false) {
      host?.setControls(identity, controls.withForcedSilence(forceSilent)) ?: false
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
    return setViewport(identity, viewport)
  }

  fun setViewport(identity: NativePlaybackIdentity, viewport: NativePlaybackViewport): Boolean =
    onMain(false) {
      host?.setViewport(identity, viewport) ?: false
    }

  /** Move an already revealed picture after its caller has retired the transport. */
  fun setRetiredViewport(viewport: NativePlaybackViewport): Boolean = onMain(false) {
    host?.setRetiredViewport(viewport) ?: false
  }

  fun stop(sessionId: String, streamHandle: String): Boolean {
    val identity = NativePlaybackIdentity.parse(sessionId, streamHandle) ?: return false
    return stop(identity, revealContent = false)
  }

  fun stop(identity: NativePlaybackIdentity, revealContent: Boolean): Boolean = onMain(false) {
    host?.unbind(identity, revealContent) ?: true
  }

  fun stopSession(sessionId: String): Boolean {
    if (!NativePlaybackIdentity.isValidSessionId(sessionId)) {
      return false
    }
    return onMain(false) {
      host?.unbindSession(sessionId, revealContent = true) ?: true
    }
  }

  fun suspendSession(sessionId: String): Boolean {
    if (!NativePlaybackIdentity.isValidSessionId(sessionId)) {
      return false
    }
    return onMain(false) {
      host?.unbindSession(sessionId, revealContent = false) ?: true
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
    host?.takeIf { it.identity == null }?.revealContent()
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
    val parent = placement.container() ?: return
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
    val parent = placement.container() ?: return null
    var playerView: PlayerView? = null
    var player: ExoPlayer? = null
    val layoutListener = View.OnLayoutChangeListener { _, left, top, right, bottom,
        oldLeft, oldTop, oldRight, oldBottom ->
      if (left != oldLeft || top != oldTop || right != oldRight || bottom != oldBottom) {
        pictureListener?.invoke()
      }
    }
    return try {
      val createdPlayerView = placement.createPlayerView(activity).apply {
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
      createdPlayerView.addOnLayoutChangeListener(layoutListener)
      placement.mount(parent, createdPlayerView)
      PlaybackHost(activity, placement, parent, createdPlayerView, createdPlayer, layoutListener).also {
        it.setPictureInPicture(pictureInPicture)
      }
    } catch (_: RuntimeException) {
      runCatching { playerView?.player = null }
      runCatching { player?.release() }
      runCatching {
        playerView?.let {
          it.removeOnLayoutChangeListener(layoutListener)
          placement.unmount(parent, it)
        }
      }
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
    private val activity: Activity,
    private val placement: NativePlaybackPlacement,
    private val parent: ViewGroup,
    private val playerView: PlayerView,
    private val player: ExoPlayer,
    private val layoutListener: View.OnLayoutChangeListener,
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
    /** The last viewport outside picture-in-picture. */
    private var lastViewport: NativePlaybackViewport? = null

    val identity: NativePlaybackIdentity?
      get() = binding?.identity

    fun bind(
      identity: NativePlaybackIdentity,
      viewport: NativePlaybackViewport,
      controls: NativePlaybackControls,
      reader: NativePlaybackReader?,
      observer: ((NativePlaybackSignal) -> Unit)?,
    ): Boolean {
      if (released || binding != null) {
        return false
      }
      lifecycleSuspended = null
      val created = PlaybackBinding(identity, player, controls, observer)
      binding = created
      return try {
        player.addListener(created)
        player.addAnalyticsListener(created)
        val dataSourceFactory = DataSource.Factory { NativePlaybackDataSource(identity, reader) }
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
        revealContent()
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
      unbind(current.identity, revealContent = false)
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

    fun setRetiredViewport(viewport: NativePlaybackViewport): Boolean {
      if (released || binding != null) return false
      setViewport(viewport)
      return true
    }

    fun unbind(
      identity: NativePlaybackIdentity,
      revealContent: Boolean,
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
      val fullscreen = placement.televisionFullscreen
      if (revealContent || (fullscreen != null && fullscreen != lastViewport?.fullscreen)) {
        revealContent()
      }
      return true
    }

    fun unbindSession(
      sessionId: String,
      revealContent: Boolean,
    ): Boolean {
      if (lifecycleSuspended?.first?.sessionId == sessionId) {
        lifecycleSuspended = null
      }
      val current = binding ?: return true
      if (current.identity.sessionId != sessionId) {
        return false
      }
      return unbind(current.identity, revealContent)
    }

    fun pauseForLifecycle() {
      if (released) {
        return
      }
      binding?.pauseForLifecycle()
    }

    fun revealContent() {
      // Picture-in-picture shows only the picture; leaving it decides.
      if (!released && !pictureInPicture) {
        placement.revealContent(parent, playerView)
      }
    }

    fun setPictureInPicture(active: Boolean) {
      if (released || pictureInPicture == active) {
        return
      }
      pictureInPicture = active
      if (active) {
        placement.fillWindow(parent, playerView)
        return
      }
      // A placement made inside picture-in-picture was for the small window.
      val last = lastViewport
      if (binding != null && last != null) {
        placement.place(parent, playerView, last)
      } else {
        revealContent()
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
      runCatching {
        playerView.removeOnLayoutChangeListener(layoutListener)
        placement.unmount(parent, playerView)
      }
    }

    private fun setViewport(viewport: NativePlaybackViewport) {
      if (released) {
        return
      }
      if (pictureInPicture) {
        placement.fillWindow(parent, playerView)
        return
      }
      lastViewport = viewport
      placement.place(parent, playerView, viewport)
    }

    private fun resetPlayer() {
      runCatching { player.stop() }
      runCatching { player.clearMediaItems() }
    }
  }

  private class PlaybackBinding(
    val identity: NativePlaybackIdentity,
    private val player: ExoPlayer,
    initialControls: NativePlaybackControls,
    private val observer: ((NativePlaybackSignal) -> Unit)?,
  ) : Player.Listener, AnalyticsListener {
    private var phase = if (initialControls.paused) Phase.PAUSED else Phase.STARTING
    private var controls = initialControls
    private val renderedFrames = NativePlaybackFrameCounter()
    private var droppedFrames = 0L
    private var audioTracks = NativePlaybackAudioTracks.UNKNOWN
    private var audioDecoder: String? = null
    private var audioPassthrough = false
    private var released = false
    private var firstFrameReported = false

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
      val decoded = renderedFrames.value().coerceAtMost(MAX_SAFE_COUNTER)
      val dropped = droppedFrames.coerceAtMost(MAX_SAFE_COUNTER)
      val buffered = bufferedDuration.coerceAtMost(MAX_SAFE_COUNTER)
      val silent = controls.muted || controls.volume == 0.0f
      val audio = nativePlaybackAudio(audioTracks, audioDecoder, audioPassthrough).wire
      return """{"state":"${phase.wire}","decodedFrames":$decoded,"droppedFrames":$dropped,"bufferedDurationMs":$buffered,"silent":$silent,"audio":"$audio"}"""
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
      if (playbackState == Player.STATE_ENDED) {
        observer?.invoke(NativePlaybackSignal.NATURAL_END)
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
        observer?.let { notify ->
          val interrupted = generateSequence(error.cause) { it.cause }
            .any { it is NativePlaybackReadInterrupted }
          notify(if (interrupted) NativePlaybackSignal.STREAM_INTERRUPTED else NativePlaybackSignal.PLAYER_ERROR)
        }
      }
    }

    override fun onRenderedFirstFrame() {
      if (!released && !firstFrameReported) {
        firstFrameReported = true
        observer?.invoke(NativePlaybackSignal.FIRST_FRAME)
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
}

@OptIn(UnstableApi::class)
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
