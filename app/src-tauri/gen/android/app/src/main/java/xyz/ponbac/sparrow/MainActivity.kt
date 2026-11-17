package xyz.ponbac.sparrow

import android.app.UiModeManager
import android.content.res.Configuration
import android.os.Bundle
import android.graphics.Color
import android.os.Looper
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.activity.SystemBarStyle
import androidx.annotation.Keep
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import java.util.concurrent.TimeUnit

class MainActivity : TauriActivity() {
  private var documentFullscreen = false
  private var webView: WebView? = null
  private var started = false
  private var playerBridge: AndroidPlayerBridge? = null
  private var televisionControls: AndroidTvControls? = null
  internal val televisionFullscreen: Boolean?
    get() = if (televisionControls == null) null else documentFullscreen
  private var audioFocus: PlaybackAudioFocus? = null
  private var pictureInPicture: PictureInPicturePlayback? = null
  private val nativePlayback by lazy {
    NativePlaybackController(
      this,
      forceSilent =
        BuildConfig.DEBUG && intent?.getBooleanExtra(ACCEPTANCE_SILENT_EXTRA, false) == true,
    )
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    if (BuildConfig.DEBUG) {
      WebView.setWebContentsDebuggingEnabled(true)
    }
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.dark(Color.BLACK),
    )
    super.onCreate(savedInstanceState)
    val content = findViewById<View>(android.R.id.content)
    content.setBackgroundColor(Color.BLACK)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      // Keep WebView controls clear of system bars, cutouts, and the keyboard.
      // Native video already measures its position relative to this WebView.
      val insetTypes = if (documentFullscreen) {
        WindowInsetsCompat.Type.ime() or WindowInsetsCompat.Type.displayCutout()
      } else {
        WindowInsetsCompat.Type.systemBars() or
          WindowInsetsCompat.Type.displayCutout() or
          WindowInsetsCompat.Type.ime()
      }
      val safe = insets.getInsets(insetTypes)
      view.setPadding(safe.left, safe.top, safe.right, safe.bottom)
      WindowInsetsCompat.CONSUMED
    }
    ViewCompat.requestApplyInsets(content)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    val focus = PlaybackAudioFocus(this) { playerBridge?.send(AndroidPlayerEvent.PAUSE) }
    val window = PictureInPicturePlayback(this, nativePlayback) { command ->
      playerBridge?.send(command)
    }
    audioFocus = focus
    pictureInPicture = window
    val setPlayback = { playback: AndroidPlayerPlayback ->
      window.setPlayback(playback)
      focus.setPlaying(playback == AndroidPlayerPlayback.PLAYING)
    }
    val bridge = AndroidPlayerBridge(
      webView,
      pictureInPictureSupported = window.supported,
      isInPictureInPicture = { isInPictureInPictureMode },
    ) { request ->
      when (request) {
        is AndroidPlayerRequest.Playback -> setPlayback(request.playback)
        AndroidPlayerRequest.EnterPictureInPicture ->
          if (!window.enter()) {
            playerBridge?.send(AndroidPlayerEvent.PICTURE_IN_PICTURE_REJECTED)
          }
        // A new page has no Playback Session until its player says so.
        AndroidPlayerRequest.Ready -> setPlayback(AndroidPlayerPlayback.NONE)
        AndroidPlayerRequest.TelevisionDefaultBack -> televisionControls?.defaultBack()
        AndroidPlayerRequest.TelevisionPageLeaving -> Unit // Handled by the bridge.
      }
    }
    playerBridge = bridge
    WebViewFullscreen(this, webView) { fullscreen ->
      val changed = documentFullscreen != fullscreen
      documentFullscreen = fullscreen
      if (changed && televisionControls != null) nativePlayback.revealRetiredPicture()
      applyFullscreenSystemBars()
      ViewCompat.requestApplyInsets(findViewById(android.R.id.content))
    }
    val mode = getSystemService(UiModeManager::class.java)?.currentModeType
    if (
      mode == Configuration.UI_MODE_TYPE_TELEVISION &&
      WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) &&
      WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)
    ) {
      televisionControls = AndroidTvControls(this, webView, bridge)
    }
  }

  override fun dispatchKeyEvent(event: KeyEvent): Boolean =
    televisionControls?.dispatchKeyEvent(event) == true || super.dispatchKeyEvent(event)

  private fun applyFullscreenSystemBars() {
    val controller = WindowCompat.getInsetsController(window, window.decorView)
    controller.systemBarsBehavior =
      WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
    if (documentFullscreen) {
      controller.hide(WindowInsetsCompat.Type.systemBars())
    } else {
      controller.show(WindowInsetsCompat.Type.systemBars())
    }
  }

  // A Playback Session lives while the picture can be seen: from onStart to
  // onStop, not onResume to onPause. Picture-in-picture, split screen and a
  // dialog over the app pause the Activity and leave the picture on screen.
  // Rust owns final stream teardown; see `android_lifecycle`.

  override fun onStart() {
    super.onStart()
    nativePlayback.resumeForLifecycle()
    clearPlaybackKeepScreenOn()
    // Back on screen in picture-in-picture (the screen was off) there is no
    // onResume to wake the page.
    webView?.onResume()
    started = true
    reportPlaybackVisibility(true)
    // The page can miss an event that reaches its WebView while it wakes.
    // Once the WebView has drawn again it can hear, so say it once more; the
    // page ignores a repeated resume.
    val view = webView
    if (view != null && WebViewFeature.isFeatureSupported(WebViewFeature.VISUAL_STATE_CALLBACK)) {
      WebViewCompat.postVisualStateCallback(view, 0) {
        if (started) {
          reportPlaybackVisibility(true)
        }
      }
    }
  }

  override fun onResume() {
    super.onResume()
    applyFullscreenSystemBars()
  }

  override fun onPause() {
    super.onPause()
    // wry pauses the WebView here, which hides the page and throttles its
    // timers. The page drives the picture while it is still on screen.
    webView?.onResume()
  }

  override fun onStop() {
    started = false
    // Pause and hide immediately; Rust then cancels a blocked DataSource read
    // before release.
    nativePlayback.pauseForLifecycle()
    clearPlaybackKeepScreenOn()
    webView?.onPause()
    reportPlaybackVisibility(false)
    super.onStop()
  }

  override fun onUserLeaveHint() {
    super.onUserLeaveHint()
    pictureInPicture?.onUserLeaveHint()
  }

  override fun onPictureInPictureModeChanged(
    isInPictureInPictureMode: Boolean,
    newConfig: Configuration,
  ) {
    super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
    nativePlayback.setPictureInPicture(isInPictureInPictureMode)
    playerBridge?.sendPictureInPicture(isInPictureInPictureMode)
  }

  /** Implemented by the app's Rust library, which wry has loaded by now. */
  private external fun reportPlaybackVisibility(visible: Boolean)

  override fun onDestroy() {
    pictureInPicture?.release()
    audioFocus?.release()
    // onStop gives Rust the first chance to cancel any blocked native read.
    // Media3 release/detach are also bounded in NativePlaybackController so a
    // missing lifecycle callback cannot indefinitely block Activity teardown.
    nativePlayback.pauseForLifecycle()
    nativePlayback.stopAll()
    super.onDestroy()
  }

  @Keep
  fun startNativePlayback(
    sessionId: String,
    streamHandle: String,
    left: Int,
    top: Int,
    width: Int,
    height: Int,
    volume: Float,
    muted: Boolean,
    fullscreen: Boolean,
  ): Boolean = nativePlayback.start(
    sessionId,
    streamHandle,
    left,
    top,
    width,
    height,
    volume,
    muted,
    fullscreen,
  )

  @Keep
  fun nativePlaybackStatus(sessionId: String, streamHandle: String): String =
    nativePlayback.status(sessionId, streamHandle)

  @Keep
  fun setNativePlaybackControls(
    sessionId: String,
    streamHandle: String,
    volume: Float,
    muted: Boolean,
    paused: Boolean,
  ): Boolean = nativePlayback.setControls(sessionId, streamHandle, volume, muted, paused)

  @Keep
  fun setNativePlaybackViewport(
    sessionId: String,
    streamHandle: String,
    left: Int,
    top: Int,
    width: Int,
    height: Int,
    fullscreen: Boolean,
  ): Boolean = nativePlayback.setViewport(
    sessionId,
    streamHandle,
    left,
    top,
    width,
    height,
    fullscreen,
  )

  @Keep
  fun stopNativePlayback(sessionId: String, streamHandle: String): Boolean =
    nativePlayback.stop(sessionId, streamHandle)

  @Keep
  fun stopNativePlaybackSession(sessionId: String): Boolean =
    nativePlayback.stopSession(sessionId)

  @Keep
  fun suspendNativePlaybackSession(sessionId: String): Boolean =
    nativePlayback.suspendSession(sessionId)

  @Keep
  fun suspendAllNativePlayback(): Boolean = nativePlayback.suspendAll()

  @Keep
  fun stopAllNativePlayback(): Boolean = nativePlayback.stopAll()

  @Keep
  fun setPlaybackKeepScreenOn(active: Boolean): Boolean {
    if (Looper.myLooper() == Looper.getMainLooper()) {
      return applyPlaybackKeepScreenOn(active)
    }

    val request = NativePlaybackMainThreadRequest(KeepScreenOnOutcome(false, false))
    return try {
      runOnUiThread {
        request.execute(
          operation = {
            val alreadySet =
              window.attributes.flags and WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON != 0
            KeepScreenOnOutcome(
              applied = applyPlaybackKeepScreenOn(active),
              addedWindowFlag = active && !alreadySet,
            )
          },
          rollback = { outcome ->
            if (outcome.applied && outcome.addedWindowFlag) {
              window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
          },
        )
      }
      request.await(2, TimeUnit.SECONDS).applied
    } catch (_: RuntimeException) {
      request.await(0, TimeUnit.NANOSECONDS).applied
    }
  }

  private data class KeepScreenOnOutcome(
    val applied: Boolean,
    val addedWindowFlag: Boolean,
  )

  private fun applyPlaybackKeepScreenOn(active: Boolean): Boolean = try {
    if (active) {
      window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    } else {
      clearPlaybackKeepScreenOn()
    }
    true
  } catch (_: RuntimeException) {
    false
  }

  private fun clearPlaybackKeepScreenOn() {
    // Chromium marks the WebView itself while media is playing. Clearing only
    // the Window lets the next view traversal immediately restore the flag.
    clearViewKeepScreenOn(window.decorView)
    window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
  }

  private fun clearViewKeepScreenOn(view: View) {
    view.keepScreenOn = false
    if (view is ViewGroup) {
      for (index in 0 until view.childCount) {
        clearViewKeepScreenOn(view.getChildAt(index))
      }
    }
  }

}
