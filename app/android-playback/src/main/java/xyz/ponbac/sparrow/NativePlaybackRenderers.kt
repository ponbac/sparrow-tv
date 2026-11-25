package xyz.ponbac.sparrow

import android.content.Context
import android.os.Handler
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.Renderer
import androidx.media3.exoplayer.audio.AudioRendererEventListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.mediacodec.MediaCodecSelector
import io.github.anilbeesetti.nextlib.media3ext.ffdecoder.FfmpegAudioRenderer

/**
 * Media3's renderers plus a software audio decoder. Devices ship without
 * decoders for some broadcast codecs (AC-3, E-AC-3, MPEG audio layer II), and
 * Media3 then plays the picture in silence. The bundled decoder turns those
 * into PCM for the same audio sink.
 */
@OptIn(UnstableApi::class)
internal class NativePlaybackRenderersFactory(context: Context) : DefaultRenderersFactory(context) {
  override fun buildAudioRenderers(
    context: Context,
    extensionRendererMode: Int,
    mediaCodecSelector: MediaCodecSelector,
    enableDecoderFallback: Boolean,
    audioSink: AudioSink,
    eventHandler: Handler,
    eventListener: AudioRendererEventListener,
    out: ArrayList<Renderer>,
  ) {
    super.buildAudioRenderers(
      context,
      extensionRendererMode,
      mediaCodecSelector,
      enableDecoderFallback,
      audioSink,
      eventHandler,
      eventListener,
      out,
    )
    // After the device's own decoders: of two renderers that can play a
    // track, Media3 takes the first, so this one plays only what they cannot.
    out.add(FfmpegAudioRenderer(eventHandler, eventListener, audioSink))
  }
}
