package xyz.ponbac.sparrow

import android.app.PendingIntent
import android.app.PictureInPictureParams
import android.app.RemoteAction
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.Rect
import android.graphics.drawable.Icon
import android.os.Build
import android.util.Rational
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat

/**
 * Shrinks the playing picture into Android's picture-in-picture window when
 * the viewer leaves the app or asks for it, with one Play or Pause button.
 * The window holds the whole Activity; `NativePlaybackController` makes the
 * picture fill it.
 */
internal class PictureInPicturePlayback(
  private val activity: MainActivity,
  private val nativePlayback: NativePlaybackController,
  private val onCommand: (String) -> Unit,
) {
  /** Android 8+ on a device that offers picture-in-picture at all. */
  val supported: Boolean =
    Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
      activity.packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)

  private var playback = AndroidPlayerPlayback.NONE
  private var released = false
  private var lastParamsKey: ParamsKey? = null

  private data class ParamsKey(
    val playback: AndroidPlayerPlayback,
    val aspect: Pair<Int, Int>,
    val bounds: Rect?,
  )

  private val receiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
      when (intent.action) {
        ACTION_PLAY -> onCommand(AndroidPlayerEvent.PLAY)
        ACTION_PAUSE -> onCommand(AndroidPlayerEvent.PAUSE)
      }
    }
  }

  init {
    if (supported) {
      ContextCompat.registerReceiver(
        activity,
        receiver,
        IntentFilter().apply {
          addAction(ACTION_PLAY)
          addAction(ACTION_PAUSE)
        },
        ContextCompat.RECEIVER_NOT_EXPORTED,
      )
      nativePlayback.setPictureListener { updateParams() }
    }
  }

  fun setPlayback(next: AndroidPlayerPlayback) {
    if (playback == next) {
      return
    }
    playback = next
    updateParams()
  }

  /** The viewer pressed the button: a playing or paused picture may shrink. */
  fun enter(): Boolean {
    if (!supported || playback == AndroidPlayerPlayback.NONE) {
      return false
    }
    return enterNow()
  }

  /**
   * Android 8 to 11 has no auto-enter: leaving for Home or Recents with a
   * playing picture enters from here, before the Activity pauses. From
   * Android 12 auto-enter covers going Home, and the system refuses an
   * explicit entry while it switches straight to another app.
   */
  fun onUserLeaveHint() {
    if (
      supported &&
      Build.VERSION.SDK_INT < Build.VERSION_CODES.S &&
      playback == AndroidPlayerPlayback.PLAYING &&
      !activity.isInPictureInPictureMode
    ) {
      enterNow()
    }
  }

  fun release() {
    if (!supported || released) {
      return
    }
    released = true
    nativePlayback.setPictureListener(null)
    runCatching { activity.unregisterReceiver(receiver) }
  }

  private fun enterNow(): Boolean {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
      return false
    }
    // Refused when the viewer turned picture-in-picture off for the app.
    return try {
      activity.enterPictureInPictureMode(params())
    } catch (_: RuntimeException) {
      false
    }
  }

  private fun updateParams() {
    if (!supported || released || Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
      return
    }
    val key = ParamsKey(
      playback = playback,
      aspect = nativePlayback.videoAspect(),
      bounds = nativePlayback.pictureBounds()?.let { Rect(it) },
    )
    if (key == lastParamsKey) {
      return
    }
    if (runCatching { activity.setPictureInPictureParams(params(key)) }.isSuccess) {
      lastParamsKey = key
    }
  }

  @RequiresApi(Build.VERSION_CODES.O)
  private fun params(key: ParamsKey? = null): PictureInPictureParams {
    val (numerator, denominator) = key?.aspect ?: nativePlayback.videoAspect()
    val builder = PictureInPictureParams.Builder()
      .setAspectRatio(Rational(numerator, denominator))
      .setActions(listOfNotNull(action()))
    // Lets the window grow out of the picture's place rather than the
    // whole screen.
    (key?.bounds ?: nativePlayback.pictureBounds())?.let(builder::setSourceRectHint)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      builder.setAutoEnterEnabled(playback == AndroidPlayerPlayback.PLAYING)
    }
    return builder.build()
  }

  @RequiresApi(Build.VERSION_CODES.O)
  private fun action(): RemoteAction? {
    val (action, icon, title) = when (playback) {
      AndroidPlayerPlayback.PLAYING ->
        Triple(ACTION_PAUSE, android.R.drawable.ic_media_pause, "Pause")
      AndroidPlayerPlayback.PAUSED ->
        Triple(ACTION_PLAY, android.R.drawable.ic_media_play, "Play")
      AndroidPlayerPlayback.NONE -> return null
    }
    val intent = PendingIntent.getBroadcast(
      activity,
      if (action == ACTION_PLAY) PLAY_REQUEST else PAUSE_REQUEST,
      Intent(action).setPackage(activity.packageName),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    return RemoteAction(Icon.createWithResource(activity, icon), title, title, intent)
  }

  private companion object {
    const val ACTION_PLAY = "xyz.ponbac.sparrow.PICTURE_IN_PICTURE_PLAY"
    const val ACTION_PAUSE = "xyz.ponbac.sparrow.PICTURE_IN_PICTURE_PAUSE"
    const val PLAY_REQUEST = 1
    const val PAUSE_REQUEST = 2
  }
}
