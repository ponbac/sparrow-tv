/* Diagnostic harness, not a production playback adapter.
 * Uses Sparrow's GTK3/WebKitGTK generation without the Tauri transport.
 */
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
#include <epoxy/gl.h>
#include <epoxy/egl.h>
#include <mpv/client.h>
#include <mpv/render_gl.h>
#include <stdio.h>
#include <string.h>

static GtkWidget *window, *area;
static WebKitWebView *webview;
static mpv_handle *player;
static mpv_render_context *renderer;
static gint redraw;
static unsigned renders, changed_samples;
static unsigned long previous_sample;
static int seconds;
static const char *source;

static gboolean close_window(GtkWidget *widget, GdkEvent *event, void *unused) {
    (void)widget; (void)event; (void)unused;
    /* Keep the GLArea alive until its render context has been freed. */
    gtk_main_quit();
    return TRUE;
}

static gboolean window_state(GtkWidget *widget, GdkEventWindowState *event, void *unused) {
    (void)widget; (void)unused;
    g_print("WINDOW_STATE fullscreen=%d\n", !!(event->new_window_state & GDK_WINDOW_STATE_FULLSCREEN));
    return FALSE;
}

static void command(const char *name, const char *arg) {
    const char *args[] = {name, arg, NULL};
    int result = mpv_command_async(player, 0, args);
    if (result < 0) g_print("COMMAND_ERROR %s\n", mpv_error_string(result));
}

static void *get_proc(void *unused, const char *name) {
    (void)unused;
    return (void *)eglGetProcAddress(name);
}

static void update(void *unused) {
    (void)unused;
    g_atomic_int_set(&redraw, 1);
}

static void realize(GtkGLArea *widget, void *unused) {
    (void)unused;
    gtk_gl_area_make_current(widget);
    GError *error = gtk_gl_area_get_error(widget);
    if (error) g_error("GLArea: %s", error->message);
    g_print("GL_RENDERER %s\n", glGetString(GL_RENDERER));
    mpv_opengl_init_params init = {.get_proc_address = get_proc};
    mpv_render_param params[] = {
        {MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_API_TYPE_OPENGL},
        {MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &init},
        {MPV_RENDER_PARAM_INVALID, NULL}
    };
    int result = mpv_render_context_create(&renderer, player, params);
    if (result < 0) g_error("libmpv renderer: %s", mpv_error_string(result));
    mpv_render_context_set_update_callback(renderer, update, NULL);
    command("loadfile", source);
}

static gboolean render(GtkGLArea *widget, GdkGLContext *context, void *unused) {
    (void)context; (void)unused;
    if (!renderer) return TRUE;
    GLint framebuffer;
    glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &framebuffer);
    int scale = gtk_widget_get_scale_factor(GTK_WIDGET(widget));
    mpv_opengl_fbo fbo = {
        .fbo = framebuffer,
        .w = gtk_widget_get_allocated_width(GTK_WIDGET(widget)) * scale,
        .h = gtk_widget_get_allocated_height(GTK_WIDGET(widget)) * scale
    };
    int flip = 1;
    mpv_render_param params[] = {
        {MPV_RENDER_PARAM_OPENGL_FBO, &fbo},
        {MPV_RENDER_PARAM_FLIP_Y, &flip},
        {MPV_RENDER_PARAM_INVALID, NULL}
    };
    mpv_render_context_render(renderer, params);
    /* Verify changing rendered pixels, not merely successful API calls.
     * Readback adds overhead; these are diagnostic, not benchmark numbers. */
    unsigned char pixels[32 * 32 * 4];
    glReadPixels(fbo.w / 2, fbo.h / 2, 32, 32, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    unsigned long hash = 5381;
    for (unsigned i = 0; i < sizeof(pixels); i++) hash = hash * 33 + pixels[i];
    if (hash != previous_sample) changed_samples++;
    previous_sample = hash;
    renders++;
    return TRUE;
}

static gboolean pump(void *unused) {
    (void)unused;
    if (renderer && g_atomic_int_compare_and_exchange(&redraw, 1, 0)) {
        mpv_render_context_update(renderer);
        gtk_gl_area_queue_render(GTK_GL_AREA(area));
    }
    if (player) {
        mpv_event *event;
        while ((event = mpv_wait_event(player, 0))->event_id != MPV_EVENT_NONE) {
            if (event->event_id == MPV_EVENT_FILE_LOADED ||
                event->event_id == MPV_EVENT_END_FILE ||
                event->event_id == MPV_EVENT_PLAYBACK_RESTART)
                g_print("MPV_EVENT %s\n", mpv_event_name(event->event_id));
        }
    }
    return G_SOURCE_CONTINUE;
}

static void js(const char *script) {
    webkit_web_view_evaluate_javascript(webview, script, -1, NULL, NULL, NULL, NULL, NULL);
}

static void web_message(WebKitUserContentManager *manager, WebKitJavascriptResult *result, void *unused) {
    (void)manager; (void)unused;
    char *value = jsc_value_to_string(webkit_javascript_result_get_js_value(result));
    g_print("WEB %s\n", value);
    if (player && strcmp(value, "control:stop") == 0) command("stop", NULL);
    if (player && strcmp(value, "control:start") == 0) command("loadfile", source);
    g_free(value);
}

static gboolean tick(void *unused) {
    (void)unused;
    seconds++;
    if (player) {
        const char *properties[] = {"time-pos", "decoder-frame-drop-count", "frame-drop-count", "estimated-vf-fps", "hwdec-current", "idle-active"};
        g_print("MPV_SAMPLE t=%d renders=%u changed=%u", seconds, renders, changed_samples);
        for (unsigned i = 0; i < G_N_ELEMENTS(properties); i++) {
            char *value = mpv_get_property_string(player, properties[i]);
            g_print(" %s=%s", properties[i], value ? value : "unavailable");
            mpv_free(value);
        }
        g_print("\n");
    }
    if (seconds == 6) {
        gtk_window_resize(GTK_WINDOW(window), 1024, 640);
        g_print("ACTION resize\n");
    }
    if (seconds == 10) {
        gtk_window_fullscreen(GTK_WINDOW(window));
        g_print("ACTION fullscreen\n");
    }
    if (seconds == 12) {
        gtk_window_unfullscreen(GTK_WINDOW(window));
        g_print("ACTION unfullscreen\n");
    }
    if (seconds == 14) {
        js("document.querySelector('#release').click()");
        g_print("ACTION release\n");
    }
    if (seconds == 19) {
        js("document.querySelector('#release').click()");
        g_print("ACTION release-before-replace\n");
    }
    if (seconds == 16 || seconds == 20) {
        js("document.querySelector('#restart').click()");
        g_print("ACTION %s\n", seconds == 16 ? "resume" : "replace");
    }
    if (seconds == 26) {
        js("document.querySelector('#release').click()");
        g_print("ACTION stop\n");
    }
    if (seconds == 28) {
        gtk_main_quit();
        return G_SOURCE_REMOVE;
    }
    return G_SOURCE_CONTINUE;
}

int main(int argc, char **argv) {
    if (argc != 4) { fprintf(stderr, "usage: player mse|embed PAGE_URL STREAM_URL\n"); return 2; }
    gtk_init(&argc, &argv);
    g_print("GDK_BACKEND %s GTK %u.%u.%u WEBKIT %u.%u.%u\n",
        G_OBJECT_TYPE_NAME(gdk_display_get_default()),
        gtk_get_major_version(), gtk_get_minor_version(), gtk_get_micro_version(),
        webkit_get_major_version(), webkit_get_minor_version(), webkit_get_micro_version());
    source = argv[3];
    gboolean embedded = strcmp(argv[1], "embed") == 0;
    window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(window), "Sparrow playback experiment");
    gtk_window_set_default_size(GTK_WINDOW(window), 960, 600);
    g_signal_connect(window, "delete-event", G_CALLBACK(close_window), NULL);
    g_signal_connect(window, "window-state-event", G_CALLBACK(window_state), NULL);
    WebKitUserContentManager *manager = webkit_user_content_manager_new();
    webkit_user_content_manager_register_script_message_handler(manager, "lab");
    g_signal_connect(manager, "script-message-received::lab", G_CALLBACK(web_message), NULL);
    webview = WEBKIT_WEB_VIEW(webkit_web_view_new_with_user_content_manager(manager));
    g_object_unref(manager);
    WebKitSettings *settings = webkit_web_view_get_settings(webview);
    webkit_settings_set_media_playback_requires_user_gesture(settings, FALSE);
    GdkRGBA transparent = {0, 0, 0, 0};
    webkit_web_view_set_background_color(webview, &transparent);
    if (embedded) {
        player = mpv_create();
        mpv_set_option_string(player, "vo", "libmpv");
        mpv_set_option_string(player, "ao", "null");
        mpv_set_option_string(player, "hwdec", "no");
        mpv_set_option_string(player, "config", "no");
        mpv_set_option_string(player, "terminal", "no");
        if (mpv_initialize(player) < 0) g_error("mpv init failed");
        GtkWidget *overlay = gtk_overlay_new();
        area = gtk_gl_area_new();
        gtk_gl_area_set_required_version(GTK_GL_AREA(area), 3, 2);
        gtk_gl_area_set_auto_render(GTK_GL_AREA(area), FALSE);
        g_signal_connect(area, "realize", G_CALLBACK(realize), NULL);
        g_signal_connect(area, "render", G_CALLBACK(render), NULL);
        gtk_container_add(GTK_CONTAINER(overlay), area);
        gtk_overlay_add_overlay(GTK_OVERLAY(overlay), GTK_WIDGET(webview));
        gtk_container_add(GTK_CONTAINER(window), overlay);
    } else gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(webview));
    gtk_widget_show_all(window);
    webkit_web_view_load_uri(webview, argv[2]);
    guint pump_id = g_timeout_add(8, pump, NULL);
    guint tick_id = g_timeout_add_seconds(1, tick, NULL);
    gtk_main();
    g_source_remove(pump_id);
    if (seconds < 28) g_source_remove(tick_id);
    if (renderer) {
        mpv_render_context_set_update_callback(renderer, NULL, NULL);
        gtk_gl_area_make_current(GTK_GL_AREA(area));
        mpv_render_context_free(renderer);
        renderer = NULL;
    }
    if (player) { mpv_terminate_destroy(player); player = NULL; }
    g_print("CLEANUP complete renders=%u changed=%u\n", renders, changed_samples);
    gtk_widget_destroy(window);
    return 0;
}
