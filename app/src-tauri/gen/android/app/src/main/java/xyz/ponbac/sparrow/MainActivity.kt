package xyz.ponbac.sparrow

import android.os.Bundle
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.annotation.Keep
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    if (BuildConfig.DEBUG) {
      WebView.setWebContentsDebuggingEnabled(true)
    }
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onPause() {
    clearPlaybackKeepScreenOn()
    super.onPause()
  }

  override fun onResume() {
    super.onResume()
    clearPlaybackKeepScreenOn()
  }

  @Keep
  fun setPlaybackKeepScreenOn(active: Boolean): Boolean {
    if (Looper.myLooper() == Looper.getMainLooper()) {
      return applyPlaybackKeepScreenOn(active)
    }

    val completed = CountDownLatch(1)
    val applied = AtomicBoolean(false)
    return try {
      runOnUiThread {
        try {
          applied.set(applyPlaybackKeepScreenOn(active))
        } finally {
          completed.countDown()
        }
      }
      completed.await(2, TimeUnit.SECONDS) && applied.get()
    } catch (_: InterruptedException) {
      Thread.currentThread().interrupt()
      false
    } catch (_: RuntimeException) {
      false
    }
  }

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
