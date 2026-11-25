package xyz.ponbac.sparrow

/** What the page says its Playback Session is doing, for the window around it. */
internal enum class AndroidPlayerPlayback {
  /** Starting, playing or recovering: worth keeping on screen and audible. */
  PLAYING,

  /** Paused by the viewer, or failed in a way Play can restart. */
  PAUSED,

  /** Nothing the picture-in-picture window could play or resume. */
  NONE,
}

/** A message from the page to the Android player window. */
internal sealed interface AndroidPlayerRequest {
  /** The page has loaded its bridge and wants the window's state. */
  data object Ready : AndroidPlayerRequest

  data class Playback(val playback: AndroidPlayerPlayback) : AndroidPlayerRequest

  /** The viewer pressed Picture in picture. */
  data object EnterPictureInPicture : AndroidPlayerRequest

  /** No TV screen or overlay handled Back. Let Android finish the navigation. */
  data object TelevisionDefaultBack : AndroidPlayerRequest

  /** The TV document is leaving and can no longer accept commands. */
  data object TelevisionPageLeaving : AndroidPlayerRequest

  companion object {
    fun parse(message: String): AndroidPlayerRequest? = when (message) {
      "ready" -> Ready
      "playback:playing" -> Playback(AndroidPlayerPlayback.PLAYING)
      "playback:paused" -> Playback(AndroidPlayerPlayback.PAUSED)
      "playback:none" -> Playback(AndroidPlayerPlayback.NONE)
      "enter-picture-in-picture" -> EnterPictureInPicture
      "tv:default-back" -> TelevisionDefaultBack
      "tv:page-leaving" -> TelevisionPageLeaving
      else -> null
    }
  }
}

/** The closed set of messages the Android player window sends the page. */
internal object AndroidPlayerEvent {
  fun capabilities(pictureInPicture: Boolean): String =
    if (pictureInPicture) "capabilities:picture-in-picture" else "capabilities:none"

  fun pictureInPicture(active: Boolean): String =
    if (active) "picture-in-picture:on" else "picture-in-picture:off"

  const val PICTURE_IN_PICTURE_REJECTED = "picture-in-picture:rejected"
  const val PLAY = "command:play"
  const val PAUSE = "command:pause"
  const val TOGGLE_PLAYBACK = "command:toggle"
  const val TELEVISION_BACK = "tv:back"
  const val TELEVISION_CHANNEL_UP = "tv:channel-up"
  const val TELEVISION_CHANNEL_DOWN = "tv:channel-down"
}
