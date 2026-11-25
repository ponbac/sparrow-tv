package xyz.ponbac.sparrow

import android.net.Uri
import androidx.annotation.Keep
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.BaseDataSource
import androidx.media3.datasource.DataSourceException
import androidx.media3.datasource.DataSpec
import java.io.IOException

internal fun interface NativePlaybackReader {
  fun read(identity: NativePlaybackIdentity, output: ByteArray, offset: Int, length: Int): Int
}

@OptIn(UnstableApi::class)
@Keep
internal class NativePlaybackDataSource(
  private val identity: NativePlaybackIdentity,
  private val reader: NativePlaybackReader?,
) : BaseDataSource(true) {
  private var opened = false

  override fun open(dataSpec: DataSpec): Long {
    if (opened || dataSpec.uri != PLAYBACK_URI || dataSpec.position != 0L) {
      throw DataSourceException(PlaybackException.ERROR_CODE_IO_READ_POSITION_OUT_OF_RANGE)
    }
    transferInitializing(dataSpec)
    opened = true
    transferStarted(dataSpec)
    return C.LENGTH_UNSET.toLong()
  }

  override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
    if (!opened) {
      throw IOException("native playback is not open")
    }
    if (length == 0) {
      return 0
    }
    // Shield adds host and presentation identity in its reader; installed Tauri
    // keeps the existing JNI entry point and wire arguments.
    val read = reader?.read(identity, buffer, offset, length) ?: readNativePlayback(
      identity.sessionId,
      identity.streamHandle,
      buffer,
      offset,
      length,
    )
    return when {
      read > 0 -> {
        bytesTransferred(read)
        read
      }
      read == 0 -> C.RESULT_END_OF_INPUT
      else -> throw NativePlaybackReadInterrupted()
    }
  }

  override fun getUri(): Uri? = if (opened) PLAYBACK_URI else null

  override fun close() {
    if (!opened) {
      return
    }
    opened = false
    transferEnded()
  }

  private external fun readNativePlayback(
    sessionId: String,
    streamHandle: String,
    output: ByteArray,
    offset: Int,
    length: Int,
  ): Int

  companion object {
    val PLAYBACK_URI: Uri = Uri.parse("sparrow-native://playback/live.ts")
  }
}

internal class NativePlaybackReadInterrupted : IOException("native playback became unavailable")
