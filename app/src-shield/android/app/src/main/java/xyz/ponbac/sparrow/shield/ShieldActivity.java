package xyz.ponbac.sparrow.shield;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.SparseArray;
import android.view.KeyEvent;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.widget.FrameLayout;
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;

import java.io.FileDescriptor;
import java.io.PrintWriter;
import java.util.concurrent.atomic.AtomicLong;

public final class ShieldActivity extends Activity implements SurfaceHolder.Callback {
    private static final AtomicLong NEXT_HOST_ID = new AtomicLong(1);
    private final Handler main = new Handler(Looper.getMainLooper());
    private final SparseArray<RoutedPress> hostPresses = new SparseArray<>();
    private long hostId;
    private TvTextEditor editor;
    private SourceDraft sourceDraft;
    private BackCallbacks backCallbacks;
    private ShieldPlayback playback;
    private boolean started;
    private boolean hostKeyRouting;

    static {
        System.loadLibrary("sparrow_shield");
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        hostId = NEXT_HOST_ID.getAndIncrement();

        FrameLayout root = new FrameLayout(this);
        root.setFocusableInTouchMode(true);
        SurfaceView view = new SurfaceView(this);
        view.getHolder().addCallback(this);
        root.addView(view, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        playback = new ShieldPlayback(this, root, view, hostId);
        editor = new TvTextEditor(root, this::updateBackCallbacks);
        sourceDraft = new SourceDraft(editor, new SourceDraft.Listener() {
            @Override
            public void onStatus(long session, long attempt, int status) {
                nativeSourceDraftStatus(hostId, session, attempt, status);
            }

            @Override
            public void onField(long session, long editorGeneration, int field, int event, boolean hasText) {
                nativeSourceFieldEvent(hostId, session, editorGeneration, field, event, hasText);
            }

            @Override
            public void onSave(long session, long attempt, String m3u, String epg) {
                nativeSaveSourceDraft(hostId, session, attempt, m3u, epg);
            }
        });
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            backCallbacks = new BackCallbacks(getOnBackInvokedDispatcher(), this::routeBack, editor::cancel);
        }
        nativeStart(getApplicationContext(), getDataDir().getAbsolutePath());
        nativeRegisterHost(hostId);
        setContentView(root);
        root.requestFocus();
    }

    @Override
    protected void onStart() {
        super.onStart();
        started = true;
        playback.onActivityStarted();
        nativeCatalogActive(hostId, true);
    }

    @Override
    protected void onStop() {
        started = false;
        playback.onActivityStopped();
        hostPresses.clear();
        editor.retirePendingRequests();
        editor.cancel();
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
        main.removeCallbacksAndMessages(null);
        sourceDraft.close(Long.MAX_VALUE);
        editor.close(Long.MAX_VALUE);
        if (backCallbacks != null) {
            backCallbacks.update(false, false);
        }
        playback.onActivityDestroyed();
        nativeDestroy(hostId, isFinishing());
        super.onDestroy();
    }

    public ShieldPlayback getPlaybackHost() {
        return playback;
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
        boolean down = action == KeyEvent.ACTION_DOWN;
        boolean up = action == KeyEvent.ACTION_UP;
        RoutedPress press = hostPresses.get(code);
        boolean continuesPress = press != null && event.getDownTime() == press.downTime();
        if (up || (down && !continuesPress)) {
            hostPresses.remove(code);
        }
        // Finish an existing guide press on the route that received its Down.
        if (editor.isOpen() && !(up && continuesPress)) {
            return super.dispatchKeyEvent(event);
        }
        boolean routeToHost = continuesPress ? press.routeToHost() : hostKeyRouting;
        if ((!down && !up) || !(isGuideKey(code) || (routeToHost && isPlaybackKey(code)))) {
            return super.dispatchKeyEvent(event);
        }
        if (down && !continuesPress) {
            hostPresses.put(code, new RoutedPress(event.getDownTime(), routeToHost));
        }
        if (routeToHost) {
            nativeHostKey(hostId, code, action, event.getMetaState(), event.getRepeatCount());
        } else {
            nativeKey(code, action, event.getMetaState());
        }
        return true;
    }

    @Override
    @SuppressLint("GestureBackNavigation") // Android 13+ uses BackCallbacks for the Guide and editor.
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        routeBack();
    }

    @SuppressWarnings("deprecation")
    private void routeBack() {
        editor.retirePendingRequests();
        if (editor.cancel()) {
            return;
        }
        if (hostKeyRouting) {
            nativeBack(hostId);
        } else {
            super.onBackPressed();
        }
    }

    private void updateBackCallbacks() {
        if (backCallbacks != null) {
            backCallbacks.update(hostKeyRouting, editor.isOpen());
        }
    }

    /** Called from Rust with physical pixel bounds relative to the GPUI SurfaceView. */
    public void openTextEditor(long id, long generation, int mode, int x, int y, int width, int height,
                               String initialText) {
        postEditorRequest(id, generation, () -> {
            boolean opened = started && mode == TvTextEditor.QUERY
                    && editor.open(generation, mode, x, y, width, height, initialText,
                    (token, event, text) -> nativeEditorEvent(hostId, token, event, text));
            if (!opened) {
                nativeEditorEvent(hostId, generation, TvTextEditor.REJECTED, "");
            }
        });
    }

    public void closeTextEditor(long id, long generation) {
        if (id == hostId) {
            editor.retireRequest(generation);
            postForHost(id, () -> editor.close(generation));
        }
    }

    public void beginSourceDraft(long id, long session) {
        postForHost(id, () -> sourceDraft.begin(session, started));
    }

    public void openSourceField(long id, long session, long editorGeneration, int field,
                                int x, int y, int width, int height) {
        postEditorRequest(id, editorGeneration, () -> sourceDraft.open(
                session, editorGeneration, field, x, y, width, height, started));
    }

    private void postEditorRequest(long id, long generation, Runnable operation) {
        if (id == hostId) {
            editor.noteRequest(generation);
            postForHost(id, operation);
        }
    }

    public void copySourceDiagnostics(long id, long session, long attempt, String text) {
        postForHost(id, () -> {
            boolean copied = started && sourceDraft.isOpen(session) && writeDiagnostics(text);
            nativeSourceClipboardResult(hostId, session, attempt, copied);
        });
    }

    public void copyPlaybackDiagnostics(long id, long attempt, String text) {
        postForHost(id, () -> {
            boolean copied = started && writeDiagnostics(text);
            nativePlaybackClipboardResult(hostId, attempt, copied);
        });
    }

    private boolean writeDiagnostics(String text) {
        try {
            ClipboardManager clipboard = getSystemService(ClipboardManager.class);
            if (clipboard != null) {
                clipboard.setPrimaryClip(ClipData.newPlainText("Sparrow diagnostics", text));
                return true;
            }
        } catch (RuntimeException unavailable) {
            // Report failure to the requesting view without logging the clipboard request.
        }
        return false;
    }

    public void saveSourceDraft(long id, long session, long attempt) {
        postForHost(id, () -> sourceDraft.save(session, attempt, started));
    }

    public void finishSourceSave(long id, long session, long attempt, boolean succeeded) {
        postForHost(id, () -> sourceDraft.finishSave(session, attempt, succeeded));
    }

    public void closeSourceDraft(long id, long session) {
        postForHost(id, () -> sourceDraft.close(session));
    }

    /** The current guide key route stays active until the native host installs its own handler. */
    public void setHostKeyRouting(long id, boolean enabled) {
        postForHost(id, () -> {
            hostKeyRouting = enabled;
            updateBackCallbacks();
        });
    }

    public void finishFromNative(long id) {
        postForHost(id, () -> {
            if (!editor.isOpen()) {
                finish();
            }
        });
    }

    private void postForHost(long id, Runnable operation) {
        main.post(() -> {
            if (id == hostId && !isFinishing() && !isDestroyed()) {
                operation.run();
            }
        });
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

    private static boolean isPlaybackKey(int code) {
        return switch (code) {
            case KeyEvent.KEYCODE_CHANNEL_UP,
                    KeyEvent.KEYCODE_CHANNEL_DOWN,
                    KeyEvent.KEYCODE_BUTTON_R1,
                    KeyEvent.KEYCODE_BUTTON_L1,
                    KeyEvent.KEYCODE_MEDIA_PLAY,
                    KeyEvent.KEYCODE_MEDIA_PAUSE,
                    KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE,
                    KeyEvent.KEYCODE_MEDIA_STOP -> true;
            default -> false;
        };
    }

    private record RoutedPress(long downTime, boolean routeToHost) {}

    @TargetApi(Build.VERSION_CODES.TIRAMISU)
    private static final class BackCallbacks {
        private final OnBackInvokedDispatcher dispatcher;
        private final OnBackInvokedCallback host;
        private final OnBackInvokedCallback editor;
        private boolean hostRegistered;
        private boolean editorRegistered;

        BackCallbacks(OnBackInvokedDispatcher dispatcher, OnBackInvokedCallback host,
                      OnBackInvokedCallback editor) {
            this.dispatcher = dispatcher;
            this.host = host;
            this.editor = editor;
        }

        void update(boolean routeToHost, boolean editing) {
            if (hostRegistered != routeToHost) {
                if (routeToHost) {
                    dispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, host);
                } else {
                    dispatcher.unregisterOnBackInvokedCallback(host);
                }
                hostRegistered = routeToHost;
            }
            if (editorRegistered != editing) {
                if (editing) {
                    // Back dismisses the editor and IME together, before the host's inner modal.
                    dispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_OVERLAY, editor);
                } else {
                    dispatcher.unregisterOnBackInvokedCallback(editor);
                }
                editorRegistered = editing;
            }
        }
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
    private native void nativeRegisterHost(long id);
    private native void nativeCatalogActive(long id, boolean active);
    private native void nativeSurface(long id, Surface surface, float density);
    private native void nativeReleaseSurface(long id);
    private native void nativeResume(long id);
    private native void nativePause(long id);
    private native void nativeDestroy(long id, boolean finishing);
    private native void nativeKey(int keyCode, int action, int modifiers);
    private native void nativeHostKey(long id, int keyCode, int action, int modifiers, int repeatCount);
    private native void nativeBack(long id);
    private native void nativeEditorEvent(long id, long generation, int event, String text);
    private native void nativeSourceDraftStatus(long id, long session, long attempt, int status);
    private native void nativeSourceClipboardResult(long id, long session, long attempt, boolean copied);
    private native void nativePlaybackClipboardResult(long id, long attempt, boolean copied);
    private native void nativeSourceFieldEvent(long id, long session, long editorGeneration,
                                              int field, int event, boolean hasText);
    private native void nativeSaveSourceDraft(long id, long session, long attempt, String m3u, String epg);
    private native String nativeSnapshot();
}
