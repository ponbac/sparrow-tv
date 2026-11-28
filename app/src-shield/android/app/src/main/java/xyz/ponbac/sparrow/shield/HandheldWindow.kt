package xyz.ponbac.sparrow.shield

import android.graphics.Color
import android.view.MotionEvent
import android.view.SurfaceView
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/** System bars, insets and touch belong to the handheld Guide window. */
internal class HandheldWindow(
  activity: ShieldActivity,
  root: FrameLayout,
  guide: SurfaceView,
) {
  private val bars = WindowCompat.getInsetsController(activity.window, root)

  init {
    configureWindow(activity)
    root.setBackgroundColor(0xff0a111e.toInt())
    ViewCompat.setOnApplyWindowInsetsListener(root) { _, insets ->
      val safe = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout(),
      )
      val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
      activity.sendInsets(safe.top, safe.right, safe.bottom, safe.left, ime.bottom)
      insets
    }
    ViewCompat.requestApplyInsets(root)
    guide.setOnTouchListener { _, event ->
      when (event.actionMasked) {
        MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP, MotionEvent.ACTION_MOVE,
        MotionEvent.ACTION_POINTER_DOWN, MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_CANCEL -> {
          val ids = IntArray(event.pointerCount) { event.getPointerId(it) }
          val positions = FloatArray(ids.size * 2)
          for (index in ids.indices) {
            positions[index * 2] = event.getX(index)
            positions[index * 2 + 1] = event.getY(index)
          }
          activity.sendTouch(event.actionMasked, event.actionIndex, ids, positions)
        }
        else -> false
      }
    }
  }

  @Suppress("DEPRECATION")
  private fun configureWindow(activity: ShieldActivity) {
    val window = activity.window
    window.clearFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN)
    WindowCompat.setDecorFitsSystemWindows(window, false)
    window.statusBarColor = Color.TRANSPARENT
    window.navigationBarColor = Color.TRANSPARENT
    bars.isAppearanceLightStatusBars = false
    bars.isAppearanceLightNavigationBars = false
    bars.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
  }

  fun setImmersive(immersive: Boolean) {
    val types = WindowInsetsCompat.Type.systemBars()
    if (immersive) bars.hide(types) else bars.show(types)
  }
}
