package xyz.ponbac.sparrow.shield;

import android.app.Activity;
import android.content.Context;
import android.os.Bundle;
import android.view.KeyEvent;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;

import java.io.FileDescriptor;
import java.io.PrintWriter;
import java.util.concurrent.atomic.AtomicLong;

public final class ShieldActivity extends Activity implements SurfaceHolder.Callback {
    private static final AtomicLong NEXT_HOST_ID = new AtomicLong(1);
    private long hostId;

    static {
        System.loadLibrary("sparrow_shield");
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        hostId = NEXT_HOST_ID.getAndIncrement();
        nativeStart(getApplicationContext(), getDataDir().getAbsolutePath());

        SurfaceView view = new SurfaceView(this);
        view.getHolder().addCallback(this);
        setContentView(view);
    }

    @Override
    protected void onStart() {
        super.onStart();
        nativeCatalogActive(hostId, true);
    }

    @Override
    protected void onStop() {
        nativeCatalogActive(hostId, false);
        super.onStop();
    }

    @Override
    protected void onResume() {
        super.onResume();
        nativeResume(hostId);
    }

    @Override
    protected void onPause() {
        nativePause(hostId);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        nativeDestroy(hostId);
        super.onDestroy();
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        sendSurface(holder);
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        sendSurface(holder);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        nativeReleaseSurface(hostId);
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        int code = event.getKeyCode();
        int action = event.getAction();
        if ((action == KeyEvent.ACTION_DOWN || action == KeyEvent.ACTION_UP) && isGuideKey(code)) {
            nativeKey(code, action, event.getMetaState());
            return true;
        }
        return super.dispatchKeyEvent(event);
    }

    private static boolean isGuideKey(int code) {
        return switch (code) {
            case KeyEvent.KEYCODE_DPAD_LEFT,
                    KeyEvent.KEYCODE_DPAD_RIGHT,
                    KeyEvent.KEYCODE_DPAD_UP,
                    KeyEvent.KEYCODE_DPAD_DOWN,
                    KeyEvent.KEYCODE_DPAD_CENTER,
                    KeyEvent.KEYCODE_ENTER -> true;
            default -> false;
        };
    }

    @Override
    public void dump(String prefix, FileDescriptor fd, PrintWriter writer, String[] args) {
        if (args != null) {
            for (String arg : args) {
                if ("--sparrow-guide".equals(arg)) {
                    writer.print(prefix);
                    writer.println(nativeSnapshot());
                    return;
                }
            }
        }
        super.dump(prefix, fd, writer, args);
    }

    private void sendSurface(SurfaceHolder holder) {
        nativeSurface(hostId, holder.getSurface(), getResources().getDisplayMetrics().density);
    }

    private native void nativeStart(Context applicationContext, String dataDir);
    private native void nativeCatalogActive(long id, boolean active);
    private native void nativeSurface(long id, Surface surface, float density);
    private native void nativeReleaseSurface(long id);
    private native void nativeResume(long id);
    private native void nativePause(long id);
    private native void nativeDestroy(long id);
    private native void nativeKey(int keyCode, int action, int modifiers);
    private native String nativeSnapshot();
}
