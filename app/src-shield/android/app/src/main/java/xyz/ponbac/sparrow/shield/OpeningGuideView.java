package xyz.ponbac.sparrow.shield;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.view.View;

/** Static counterpart of the first GPUI arrival frame, in the Guide's 1920-wide coordinates. */
final class OpeningGuideView extends View {
    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Drawable mark;

    OpeningGuideView(Context context) {
        super(context);
        setBackgroundColor(0xff0a111e);
        setContentDescription("Sparrow");
        paint.setColor(0xffeef3f8);
        paint.setTextSize(72f);
        paint.setTypeface(Typeface.createFromAsset(context.getAssets(), "sparrow-guide-80-700.ttf"));
        mark = context.getDrawable(R.drawable.opening_signal);
        mark.setBounds(696, -60, 936, 60);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        float scale = getWidth() / 1920f;
        canvas.translate(0, getHeight() / 2f - 165f * scale);
        canvas.scale(scale, scale);

        mark.draw(canvas);
        canvas.drawText("Sparrow", 958f, -(paint.ascent() + paint.descent()) / 2f, paint);
    }
}
