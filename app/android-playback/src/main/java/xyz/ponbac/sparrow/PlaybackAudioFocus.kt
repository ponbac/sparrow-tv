package xyz.ponbac.sparrow

import android.content.Context
import android.media.AudioManager
import android.os.Build
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.audio.AudioFocusRequestCompat
import androidx.media3.common.audio.AudioManagerCompat
import androidx.media3.common.util.UnstableApi

/**
 * Holds audio focus while the host reports a playing Playback Session, so
 * picture-in-picture does not play over music or a call. Losing focus, or
 * being refused it, asks the host to pause as if the viewer had: live
 * playback pauses by releasing the stream, and the viewer resumes it at the
 * live edge. Brief notification sounds duck it instead (Android 8+ does that).
 */
@OptIn(UnstableApi::class)
internal class PlaybackAudioFocus(
  context: Context,
  private val onLost: () -> Unit,
) {
  private val audioManager = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
  private var request: Request? = null

  fun setPlaying(playing: Boolean) {
    if (playing && request == null) {
      val token = Any()
      val next = AudioFocusRequestCompat.Builder(AudioManagerCompat.AUDIOFOCUS_GAIN)
        .setAudioAttributes(
          AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MOVIE)
            .build(),
        )
        .setOnAudioFocusChangeListener { change -> onFocusChange(token, change) }
        .build()
      val granted = runCatching {
        AudioManagerCompat.requestAudioFocus(audioManager, next)
      }.getOrDefault(AudioManager.AUDIOFOCUS_REQUEST_FAILED)
      if (granted == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
        request = Request(token, next)
      } else {
        onLost()
      }
    } else if (!playing) {
      abandon()
    }
  }

  fun release() = abandon()

  private fun onFocusChange(token: Any, change: Int) {
    if (request?.token !== token) return
    val lost = when (change) {
      AudioManager.AUDIOFOCUS_LOSS, AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> true
      // Android ducks for the app from 8.0. Before that, quieter is the
      // app's job; pausing is the one way it has to step aside.
      AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK ->
        Build.VERSION.SDK_INT < Build.VERSION_CODES.O
      else -> false
    }
    if (lost) {
      abandon()
      onLost()
    }
  }

  private fun abandon() {
    val current = request ?: return
    request = null
    runCatching { AudioManagerCompat.abandonAudioFocusRequest(audioManager, current.platform) }
  }

  private data class Request(val token: Any, val platform: AudioFocusRequestCompat)
}
