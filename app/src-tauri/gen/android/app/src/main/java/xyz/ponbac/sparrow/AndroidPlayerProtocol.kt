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

  companion object {
    fun parse(message: String): AndroidPlayerRequest? = when (message) {
      "ready" -> Ready
      "playback:playing" -> Playback(AndroidPlayerPlayback.PLAYING)
      "playback:paused" -> Playback(AndroidPlayerPlayback.PAUSED)
      "playback:none" -> Playback(AndroidPlayerPlayback.NONE)
      "enter-picture-in-picture" -> EnterPictureInPicture
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
}

/**
 * The picture-in-picture window's shape for a video of this size, as a
 * numerator and denominator. Android refuses ratios beyond 2.39:1 either way,
 * so wider or taller pictures are letterboxed at that limit, and a picture of
 * unknown size gets 16:9.
 */
internal fun pictureInPictureAspect(
  width: Int,
  height: Int,
  pixelWidthHeightRatio: Float,
): Pair<Int, Int> {
  if (width <= 0 || height <= 0 || !(pixelWidthHeightRatio > 0f)) {
    return 16 to 9
  }
  val ratio = width.toDouble() * pixelWidthHeightRatio / height
  if (!ratio.isFinite()) {
    return 16 to 9
  }
  val numerator = Math.round(ratio * ASPECT_DENOMINATOR).toInt()
  return when {
    numerator * 100L >= ASPECT_DENOMINATOR * 239L -> 239 to 100
    numerator * 239L <= ASPECT_DENOMINATOR * 100L -> 100 to 239
    else -> numerator to ASPECT_DENOMINATOR
  }
}

private const val ASPECT_DENOMINATOR = 1_000
