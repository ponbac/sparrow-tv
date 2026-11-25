package xyz.ponbac.sparrow.shield;

import android.animation.ValueAnimator;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.os.SystemClock;
import android.view.View;

/** The first Activity can draw while GPUI is still opening its first window. */
final class OpeningGuideView extends View {
    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final long start = SystemClock.uptimeMillis();

    OpeningGuideView(Context context) {
        super(context);
        setBackgroundColor(0xff0a111e);
        setContentDescription("Sparrow. Opening the guide.");
        paint.setTextAlign(Paint.Align.CENTER);
        paint.setStrokeCap(Paint.Cap.ROUND);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        float scale = getWidth() / 1920f;
        canvas.translate(getWidth() / 2f, getHeight() / 2f);
        canvas.scale(scale, scale);
        boolean moving = ValueAnimator.areAnimatorsEnabled();
        double phase = moving ? (SystemClock.uptimeMillis() - start) / 1100.0 : 0;
        paint.setColor(0xff9fe0ff);
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(6.5f);
        arcs(canvas, 24f, phase);
        arcs(canvas, 42f, phase + 0.8);
        paint.setStyle(Paint.Style.FILL);
        paint.setAlpha(255);
        canvas.drawCircle(0, -74f, 10.5f, paint);
        paint.setTypeface(Typeface.DEFAULT_BOLD);
        paint.setTextSize(54f);
        paint.setColor(0xffeef3f8);
        canvas.drawText("Sparrow", 0, 20f, paint);
        paint.setTypeface(Typeface.DEFAULT);
        paint.setTextSize(22f);
        paint.setColor(0xffb0b5bc);
        canvas.drawText("Opening the guide", 0, 63f, paint);
        if (moving) postInvalidateDelayed(42);
    }

    private void arcs(Canvas canvas, float radius, double phase) {
        double pulse = (1 - Math.cos(2 * Math.PI * phase)) / 2;
        paint.setAlpha((int) (82 + 173 * pulse));
        canvas.drawArc(-radius, -74f - radius, radius, -74f + radius, -36, 72, false, paint);
        canvas.drawArc(-radius, -74f - radius, radius, -74f + radius, 144, 72, false, paint);
    }
}
