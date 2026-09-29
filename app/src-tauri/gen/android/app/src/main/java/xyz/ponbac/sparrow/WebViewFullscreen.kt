package xyz.ponbac.sparrow

import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

/** Follows document fullscreen independently of the native Playback Session lifetime. */
internal class WebViewFullscreen(
  activity: MainActivity,
  private val webView: WebView,
  private val onChanged: (Boolean) -> Unit,
) {
  private val back = object : OnBackPressedCallback(false) {
    override fun handleOnBackPressed() {
      webView.evaluateJavascript(
        "if (document.fullscreenElement) document.exitFullscreen().catch(() => {});",
        null,
      )
    }
  }

  init {
    activity.onBackPressedDispatcher.addCallback(activity, back)
    if (
      WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) &&
      WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)
    ) {
      // Only the bundled top-level app can change the window's immersive state.
      val origins = setOf("http://tauri.localhost", "https://tauri.localhost")
      WebViewCompat.addWebMessageListener(webView, "sparrowFullscreen", origins) {
          _, message, _, isMainFrame, _ ->
        if (isMainFrame && message.type == WebMessageCompat.TYPE_STRING) {
          val fullscreen = when (message.data) {
            "enter" -> true
            "exit" -> false
            else -> null
          }
          if (fullscreen != null) {
            back.isEnabled = fullscreen
            onChanged(fullscreen)
          }
        }
      }
      WebViewCompat.addDocumentStartJavaScript(
        webView,
        """
          (() => {
            if (window !== window.top) return;
            const update = () => sparrowFullscreen.postMessage(document.fullscreenElement ? 'enter' : 'exit');
            document.addEventListener('fullscreenchange', update);
            window.addEventListener('pagehide', () => sparrowFullscreen.postMessage('exit'));
            update();
          })();
        """.trimIndent(),
        origins,
      )
    }
  }
}
