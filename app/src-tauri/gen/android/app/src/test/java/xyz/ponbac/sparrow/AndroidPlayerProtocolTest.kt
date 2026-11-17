package xyz.ponbac.sparrow

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AndroidPlayerProtocolTest {
  @Test
  fun pageMessagesOutsideTheClosedSetAreIgnored() {
    assertEquals(
      AndroidPlayerRequest.Playback(AndroidPlayerPlayback.PAUSED),
      AndroidPlayerRequest.parse("playback:paused"),
    )
    assertEquals(
      AndroidPlayerRequest.EnterPictureInPicture,
      AndroidPlayerRequest.parse("enter-picture-in-picture"),
    )
    assertNull(AndroidPlayerRequest.parse("playback:PLAYING"))
    assertNull(AndroidPlayerRequest.parse("{\"type\":\"ready\"}"))
    assertNull(AndroidPlayerRequest.parse(""))
  }

  @Test
  fun pictureInPictureKeepsTheVideoShapeWithinAndroidsLimits() {
    assertEquals(1_778 to 1_000, pictureInPictureAspect(1280, 720, 1f))
    // Anamorphic SD: 720x576 shown 16:9.
    assertEquals(1_778 to 1_000, pictureInPictureAspect(720, 576, 64f / 45f))
    assertEquals(239 to 100, pictureInPictureAspect(3000, 1000, 1f))
    assertEquals(100 to 239, pictureInPictureAspect(400, 1000, 1f))
    // Rounding must not land just outside the limit.
    assertEquals(100 to 239, pictureInPictureAspect(10_000, 23_895, 1f))
  }

  @Test
  fun aPictureOfUnknownSizeGetsSixteenByNine() {
    assertEquals(16 to 9, pictureInPictureAspect(0, 720, 1f))
    assertEquals(16 to 9, pictureInPictureAspect(1280, 0, 1f))
    assertEquals(16 to 9, pictureInPictureAspect(1280, 720, 0f))
    assertEquals(16 to 9, pictureInPictureAspect(1280, 720, Float.NaN))
  }
}
