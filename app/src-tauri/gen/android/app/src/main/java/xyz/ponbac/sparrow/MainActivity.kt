package xyz.ponbac.sparrow

import android.os.Bundle
import android.graphics.Color
import android.os.Looper
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
import java.util.concurrent.TimeUnit

class MainActivity : TauriActivity() {
  private var documentFullscreen = false
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
    WebViewFullscreen(this, webView) { fullscreen ->
      documentFullscreen = fullscreen
      applyFullscreenSystemBars()
      ViewCompat.requestApplyInsets(findViewById(android.R.id.content))
    }
  }

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

  override fun onPause() {
    // Rust owns final stream teardown. Pause/hide immediately here, then let
    // its lifecycle path cancel a blocked DataSource read before release.
    nativePlayback.pauseForLifecycle()
    clearPlaybackKeepScreenOn()
    super.onPause()
  }

  override fun onResume() {
    super.onResume()
    applyFullscreenSystemBars()
    nativePlayback.resumeForLifecycle()
    clearPlaybackKeepScreenOn()
  }

  override fun onDestroy() {
    // onPause gives Rust the first chance to cancel any blocked native read.
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
