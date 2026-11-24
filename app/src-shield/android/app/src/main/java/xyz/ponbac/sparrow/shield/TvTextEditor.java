package xyz.ponbac.sparrow.shield;

import android.os.Build;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.view.textclassifier.TextClassifier;
import android.widget.EditText;
import android.widget.FrameLayout;

import java.util.concurrent.atomic.AtomicLong;

/** Hosts one live field while Android owns text selection and IME composition. */
final class TvTextEditor {
    static final int QUERY = 0;
    static final int SOURCE_URI = 1;
    static final int CHANGED = 0;
    static final int COMMITTED = 1;
    static final int CANCELED = 2;
    static final int REJECTED = 3;

    interface Listener {
        void onEvent(long generation, int event, String text);
    }

    private final FrameLayout root;
    private final InputMethodManager keyboard;
    private final Runnable onActiveChanged;
    // Posting and retiring requests can happen on the GPUI thread. The field
    // itself and its generation belong to the Activity main thread.
    private final AtomicLong latestRequest = new AtomicLong();
    private final AtomicLong retiredRequest = new AtomicLong();
    private long generation;
    private Input input;

    TvTextEditor(FrameLayout root, Runnable onActiveChanged) {
        this.root = root;
        this.keyboard = root.getContext().getSystemService(InputMethodManager.class);
        this.onActiveChanged = onActiveChanged;
    }

    boolean isOpen() {
        return input != null;
    }

    boolean isOpen(long editorGeneration) {
        return input != null && generation == editorGeneration;
    }

    void noteRequest(long nextGeneration) {
        latestRequest.accumulateAndGet(nextGeneration, Math::max);
    }

    void retireRequest(long closingGeneration) {
        retiredRequest.accumulateAndGet(closingGeneration, Math::max);
    }

    void retirePendingRequests() {
        retireRequest(latestRequest.get());
    }

    boolean open(long nextGeneration, int mode, int x, int y, int width, int height,
                 String initialText, Listener listener) {
        if (nextGeneration <= retiredRequest.get() || nextGeneration < generation
                || width <= 0 || height <= 0
                || (mode != QUERY && mode != SOURCE_URI)) {
            return false;
        }
        FrameLayout.LayoutParams bounds = new FrameLayout.LayoutParams(width, height,
                Gravity.TOP | Gravity.LEFT);
        bounds.leftMargin = x;
        bounds.topMargin = y;
        if (nextGeneration == generation) {
            if (input == null || input.mode != mode) {
                return false;
            }
            input.setLayoutParams(bounds);
            return true;
        }
        if (input != null) {
            return false;
        }
        generation = nextGeneration;
        Input field = new Input(mode, listener);
        field.setInputType(InputType.TYPE_CLASS_TEXT | (mode == SOURCE_URI
                ? InputType.TYPE_TEXT_VARIATION_URI | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS : 0));
        if (mode == SOURCE_URI) {
            field.setTextClassifier(TextClassifier.NO_OP);
        }
        field.setSingleLine(true);
        field.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
        field.setSaveEnabled(false);
        field.setSaveFromParentEnabled(false);
        field.setFreezesText(false);
        field.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            field.setImportantForContentCapture(View.IMPORTANT_FOR_CONTENT_CAPTURE_NO_EXCLUDE_DESCENDANTS);
        }
        field.setText(initialText);
        field.setSelection(field.length());
        field.setOnEditorActionListener((view, action, event) -> {
            if (action != EditorInfo.IME_ACTION_DONE || event != null) {
                return false;
            }
            finish(field, COMMITTED);
            return true;
        });
        if (mode == QUERY) {
            field.addTextChangedListener(new TextWatcher() {
                @Override
                public void beforeTextChanged(CharSequence text, int start, int count, int after) {}

                @Override
                public void onTextChanged(CharSequence text, int start, int before, int count) {}

                @Override
                public void afterTextChanged(Editable text) {
                    if (input == field) {
                        listener.onEvent(nextGeneration, CHANGED, text.toString());
                    }
                }
            });
        }
        input = field;
        root.addView(field, bounds);
        field.requestFocus();
        field.post(() -> showKeyboard(field));
        onActiveChanged.run();
        return true;
    }

    void close(long closingGeneration) {
        if (closingGeneration >= generation) {
            generation = closingGeneration;
            clear();
        }
    }

    boolean cancel() {
        if (input == null) {
            return false;
        }
        finish(input, CANCELED);
        return true;
    }

    private void finish(Input field, int event) {
        if (input != field) {
            return;
        }
        long finishedGeneration = generation;
        String text = event == COMMITTED ? field.getText().toString() : "";
        clear();
        field.listener.onEvent(finishedGeneration, event, text);
    }

    private void showKeyboard(Input field) {
        if (keyboard != null && input == field && field.hasWindowFocus()) {
            keyboard.showSoftInput(field, InputMethodManager.SHOW_IMPLICIT);
        }
    }

    private void clear() {
        Input field = input;
        if (field == null) {
            return;
        }
        input = null;
        if (keyboard != null) {
            keyboard.hideSoftInputFromWindow(field.getWindowToken(), 0);
        }
        field.getText().clear();
        root.removeView(field);
        root.requestFocus();
        onActiveChanged.run();
    }

    private final class Input extends EditText {
        private final int mode;
        private final Listener listener;
        private int commitKey = KeyEvent.KEYCODE_UNKNOWN;
        private long commitDownTime;

        Input(int mode, Listener listener) {
            super(root.getContext());
            this.mode = mode;
            this.listener = listener;
        }

        @Override
        public void onWindowFocusChanged(boolean focused) {
            super.onWindowFocusChanged(focused);
            if (focused) {
                showKeyboard(this);
            }
        }

        @Override
        public boolean onKeyDown(int code, KeyEvent event) {
            if (!isCommitKey(code)) {
                return super.onKeyDown(code, event);
            }
            if (event.getRepeatCount() == 0) {
                commitKey = code;
                commitDownTime = event.getDownTime();
            }
            return true;
        }

        @Override
        public boolean onKeyUp(int code, KeyEvent event) {
            if (!isCommitKey(code)) {
                return super.onKeyUp(code, event);
            }
            boolean pressedHere = commitKey == code && commitDownTime == event.getDownTime();
            commitKey = KeyEvent.KEYCODE_UNKNOWN;
            if (pressedHere && !event.isCanceled()) {
                finish(this, COMMITTED);
            }
            return true;
        }

        @Override
        public boolean onKeyPreIme(int code, KeyEvent event) {
            if (code != KeyEvent.KEYCODE_BACK || input != this) {
                return super.onKeyPreIme(code, event);
            }
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
                finish(this, CANCELED);
            }
            return true;
        }

        private boolean isCommitKey(int code) {
            return code == KeyEvent.KEYCODE_DPAD_CENTER || code == KeyEvent.KEYCODE_ENTER
                    || code == KeyEvent.KEYCODE_NUMPAD_ENTER;
        }
    }
}
