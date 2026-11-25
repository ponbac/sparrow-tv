package xyz.ponbac.sparrow

import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.ui.PlayerView

/** Places the native picture above Tauri's WebView, using the WebView's origin. */
@OptIn(UnstableApi::class)
internal class WebViewPlaybackPlacement(private val activity: MainActivity) : NativePlaybackPlacement {
  override val televisionFullscreen: Boolean?
    get() = activity.televisionFullscreen

  override fun container(): ViewGroup? = activity.window.decorView as? ViewGroup

  override fun mount(parent: ViewGroup, playerView: PlayerView) {
    parent.addView(playerView)
  }

  override fun revealContent(parent: ViewGroup, playerView: PlayerView) {
    directWebContentChild(parent)?.bringToFront()
  }

  override fun fillWindow(parent: ViewGroup, playerView: PlayerView) {
    val current = playerView.layoutParams as? FrameLayout.LayoutParams
    if (
      current == null ||
      current.width != ViewGroup.LayoutParams.MATCH_PARENT ||
      current.height != ViewGroup.LayoutParams.MATCH_PARENT ||
      current.leftMargin != 0 ||
      current.topMargin != 0
    ) {
      playerView.layoutParams = FrameLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.MATCH_PARENT,
      )
    }
    bringPlayerToFrontIfNeeded(parent, playerView)
  }

  override fun place(
    parent: ViewGroup,
    playerView: PlayerView,
    viewport: NativePlaybackViewport,
  ) {
    val webView = findWebView(parent) ?: return
    val webLocation = IntArray(2)
    val parentLocation = IntArray(2)
    webView.getLocationInWindow(webLocation)
    parent.getLocationInWindow(parentLocation)
    val leftMargin = webLocation[0] - parentLocation[0] + viewport.left
    val topMargin = webLocation[1] - parentLocation[1] + viewport.top
    val current = playerView.layoutParams as? FrameLayout.LayoutParams
    if (
      current == null ||
      current.width != viewport.width ||
      current.height != viewport.height ||
      current.leftMargin != leftMargin ||
      current.topMargin != topMargin
    ) {
      playerView.layoutParams =
        FrameLayout.LayoutParams(viewport.width, viewport.height).apply {
          this.leftMargin = leftMargin
          this.topMargin = topMargin
        }
    }
    bringPlayerToFrontIfNeeded(parent, playerView)
  }

  private fun bringPlayerToFrontIfNeeded(parent: ViewGroup, playerView: PlayerView) {
    if (parent.childCount > 0 && parent.getChildAt(parent.childCount - 1) !== playerView) {
      playerView.bringToFront()
    }
  }

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
