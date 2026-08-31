package xyz.ponbac.sparrow

import android.os.Bundle
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.annotation.Keep
import java.util.concurrent.TimeUnit

class MainActivity : TauriActivity() {
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
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
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
