package xyz.ponbac.sparrow

import android.view.KeyEvent
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.webkit.WebViewCompat

/** Passes TV actions the WebView does not handle to the bundled page. */
internal class AndroidTvControls(
  private val activity: MainActivity,
  webView: WebView,
  private val bridge: AndroidPlayerBridge,
) {
  private val back = object : OnBackPressedCallback(true) {
    override fun handleOnBackPressed() {
      if (!bridge.send(AndroidPlayerEvent.TELEVISION_BACK)) {
        defaultBack()
      }
    }
  }

  init {
    activity.onBackPressedDispatcher.addCallback(activity, back)
    WebViewCompat.addDocumentStartJavaScript(
      webView,
      """
        (() => {
          if (window !== window.top) return;
          window.addEventListener('pagehide', (event) => {
            if (!event.persisted) sparrowAndroidPlayer.postMessage('tv:page-leaving');
          });
          const markTelevision = () => {
            document.documentElement.dataset.television = '';
          };
          if (document.documentElement) {
            markTelevision();
          } else {
            const observer = new MutationObserver(() => {
              if (!document.documentElement) return;
              observer.disconnect();
              markTelevision();
            });
            observer.observe(document, { childList: true });
          }
        })();
      """.trimIndent(),
      BUNDLED_WEBVIEW_ORIGINS,
    )
    // Wry attaches the WebView after its creation callback. Give the first
    // remote press to the focused page control, instead of focusing the view.
    webView.post { webView.requestFocus() }
  }

  /** Bypasses the page callback while Android handles Back normally. */
  fun defaultBack() {
    back.isEnabled = false
    try {
      activity.onBackPressedDispatcher.onBackPressed()
    } finally {
      back.isEnabled = true
    }
  }

  /** D-pad and Android's A/B fallbacks continue through normal input dispatch. */
  fun dispatchKeyEvent(event: KeyEvent): Boolean {
    val command = when (event.keyCode) {
      KeyEvent.KEYCODE_MEDIA_PLAY -> AndroidPlayerEvent.PLAY
      KeyEvent.KEYCODE_MEDIA_PAUSE -> AndroidPlayerEvent.PAUSE
      KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE -> AndroidPlayerEvent.TOGGLE_PLAYBACK
      KeyEvent.KEYCODE_CHANNEL_UP, KeyEvent.KEYCODE_BUTTON_R1 -> AndroidPlayerEvent.TELEVISION_CHANNEL_UP
      KeyEvent.KEYCODE_CHANNEL_DOWN, KeyEvent.KEYCODE_BUTTON_L1 -> AndroidPlayerEvent.TELEVISION_CHANNEL_DOWN
      else -> return false
    }
    val channel = command == AndroidPlayerEvent.TELEVISION_CHANNEL_UP ||
      command == AndroidPlayerEvent.TELEVISION_CHANNEL_DOWN
    if (event.action == KeyEvent.ACTION_DOWN && (event.repeatCount == 0 || channel)) {
      bridge.send(command)
    }
    return true
  }
}
