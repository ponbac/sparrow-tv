package xyz.ponbac.sparrow

import android.webkit.WebView
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

/**
 * The page's line to the Android window that holds its player: it reports the
 * Playback Session's state and asks for picture-in-picture; the window answers
 * with what it can do, its picture-in-picture mode, and commands from the
 * picture-in-picture buttons and audio focus. Only exists on Android, so the
 * page knows from its presence whose word on visibility to take.
 */
internal class AndroidPlayerBridge(
  webView: WebView,
  private val pictureInPictureSupported: Boolean,
  private val isInPictureInPicture: () -> Boolean,
  private val onRequest: (AndroidPlayerRequest) -> Unit,
) {
  private var page: JavaScriptReplyProxy? = null

  init {
    if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
      // Only the bundled top-level app talks to the player window.
      val origins = setOf("http://tauri.localhost", "https://tauri.localhost")
      WebViewCompat.addWebMessageListener(webView, "sparrowAndroidPlayer", origins) {
          _, message, _, isMainFrame, replyProxy ->
        val data = message.data
        if (!isMainFrame || message.type != WebMessageCompat.TYPE_STRING || data == null) {
          return@addWebMessageListener
        }
        val request = AndroidPlayerRequest.parse(data) ?: return@addWebMessageListener
        // A reload brings a new page and a new proxy.
        page = replyProxy
        if (request == AndroidPlayerRequest.Ready) {
          send(AndroidPlayerEvent.capabilities(pictureInPictureSupported))
          sendPictureInPicture(isInPictureInPicture())
        }
        onRequest(request)
      }
    }
  }

  fun sendPictureInPicture(active: Boolean) = send(AndroidPlayerEvent.pictureInPicture(active))

  fun send(event: String) {
    runCatching { page?.postMessage(event) }
  }
}
