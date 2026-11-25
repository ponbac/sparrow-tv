package xyz.ponbac.sparrow.shield;

/** Keeps the Source form's two raw locations on the Activity's main thread until Save. */
final class SourceDraft {
    private static final int M3U = 0;
    private static final int EPG = 1;
    private static final int BEGUN = 0;
    private static final int BEGIN_REJECTED = 1;
    private static final int SAVE_REJECTED = 2;

    interface Listener {
        void onStatus(long session, long attempt, int status);
        void onField(long session, long editorGeneration, int field, int event, boolean hasText);
        void onSave(long session, long attempt, String m3u, String epg);
    }

    private final TvTextEditor editor;
    private final Listener listener;
    private long generation;
    private long lastBeginRequest;
    private Draft draft;

    SourceDraft(TvTextEditor editor, Listener listener) {
        this.editor = editor;
        this.listener = listener;
    }

    void begin(long session, boolean started) {
        if (session == generation && session > 0) {
            // An active session has already reported Begun. A closed one stays closed.
            return;
        }
        boolean fresh = session > Math.max(generation, lastBeginRequest);
        if (fresh) {
            lastBeginRequest = session;
        }
        if (!started || !fresh) {
            listener.onStatus(session, 0, BEGIN_REJECTED);
            return;
        }
        close(session);
        draft = new Draft();
        listener.onStatus(session, 0, BEGUN);
    }

    void open(long session, long editorGeneration, int field, int x, int y, int width, int height,
              boolean started) {
        if (dismissed(session)) {
            return;
        }
        Draft current = session == generation ? draft : null;
        boolean validField = field == M3U || field == EPG;
        if (current == null || !validField) {
            listener.onField(session, editorGeneration, field, TvTextEditor.REJECTED, false);
            return;
        }
        boolean editing = editor.isOpen(current.editorGeneration);
        boolean repositioning = editing && current.editorGeneration == editorGeneration && current.field == field;
        boolean fresh = editorGeneration > current.lastEditorRequest;
        if (fresh) {
            current.lastEditorRequest = editorGeneration;
        }
        if (started && current.pendingSaveAttempt == 0 && (repositioning || (fresh && !editing))
                && editor.open(editorGeneration, TvTextEditor.SOURCE_URI, x, y, width, height,
                    current.text[field],
                    (id, event, text) -> editFinished(current, session, id, field, event, text))) {
            current.editorGeneration = editorGeneration;
            current.field = field;
            return;
        }
        listener.onField(session, editorGeneration, field,
                editor.rejectionEvent(editorGeneration, started), hasText(current.text[field]));
    }

    private void editFinished(Draft current, long session, long editorGeneration, int field,
                              int event, String text) {
        if (draft != current || current.editorGeneration != editorGeneration) {
            return;
        }
        current.editorGeneration = 0;
        if (event == TvTextEditor.COMMITTED) {
            current.text[field] = text;
        }
        listener.onField(session, editorGeneration, field, event, hasText(current.text[field]));
    }

    void save(long session, long attempt, boolean started) {
        if (dismissed(session)) {
            return;
        }
        Draft current = session == generation ? draft : null;
        if (current == null) {
            listener.onStatus(session, attempt, SAVE_REJECTED);
            return;
        }
        if (attempt > 0 && (attempt == current.lastSaveAttempt || attempt == current.pendingSaveAttempt)) {
            return;
        }
        boolean fresh = attempt > current.lastSaveAttempt;
        if (fresh) {
            current.lastSaveAttempt = attempt;
        }
        if (!started || !fresh || current.pendingSaveAttempt != 0
                || editor.isOpen(current.editorGeneration)) {
            listener.onStatus(session, attempt, SAVE_REJECTED);
            return;
        }
        current.pendingSaveAttempt = attempt;
        String epg = current.text[EPG];
        listener.onSave(session, attempt, current.text[M3U], epg.isEmpty() ? null : epg);
    }

    void finishSave(long session, long attempt, boolean succeeded) {
        if (session != generation || draft == null || attempt <= 0 || attempt != draft.pendingSaveAttempt) {
            return;
        }
        if (succeeded) {
            close(session);
        } else {
            draft.pendingSaveAttempt = 0;
        }
    }

    void close(long session) {
        if (session <= 0 || session < generation) {
            return;
        }
        generation = session;
        if (draft != null) {
            Draft closing = draft;
            draft = null;
            closing.text[M3U] = "";
            closing.text[EPG] = "";
            editor.close(closing.editorGeneration);
        }
    }

    boolean isOpen(long session) {
        return session == generation && draft != null;
    }

    private boolean dismissed(long session) {
        return session > 0 && session == generation && draft == null;
    }

    private static boolean hasText(String text) {
        return !text.trim().isEmpty();
    }

    private static final class Draft {
        final String[] text = {"", ""};
        long editorGeneration;
        long lastEditorRequest;
        int field;
        long lastSaveAttempt;
        long pendingSaveAttempt;
    }
}
