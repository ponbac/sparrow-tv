package xyz.ponbac.sparrow

import android.webkit.WebView
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

internal val BUNDLED_WEBVIEW_ORIGINS = setOf("http://tauri.localhost", "https://tauri.localhost")

/**
 * The page's line to the Android window that holds its player: it reports the
 * Playback Session's state and asks for picture-in-picture; the window answers
 * with what it can do, its picture-in-picture mode, and commands from the
 * picture-in-picture buttons, audio focus, and TV controls. Only exists on Android, so the
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
      WebViewCompat.addWebMessageListener(webView, "sparrowAndroidPlayer", BUNDLED_WEBVIEW_ORIGINS) {
          _, message, _, isMainFrame, replyProxy ->
        val data = message.data
        if (!isMainFrame || message.type != WebMessageCompat.TYPE_STRING || data == null) {
          return@addWebMessageListener
        }
        val request = AndroidPlayerRequest.parse(data) ?: return@addWebMessageListener
        if (request == AndroidPlayerRequest.TelevisionPageLeaving) {
          if (page === replyProxy) page = null
          return@addWebMessageListener
        }
        if (request == AndroidPlayerRequest.Ready) {
          // A reload brings a new page and a new proxy. Hand it commands once
          // it has installed its listener for replies.
          page = replyProxy
          send(AndroidPlayerEvent.capabilities(pictureInPictureSupported))
          sendPictureInPicture(isInPictureInPicture())
        }
        onRequest(request)
      }
    }
  }

  fun sendPictureInPicture(active: Boolean) = send(AndroidPlayerEvent.pictureInPicture(active))

  fun send(event: String): Boolean {
    val recipient = page ?: return false
    return runCatching { recipient.postMessage(event) }.isSuccess
  }
}
