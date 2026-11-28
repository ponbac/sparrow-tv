#include <mpv/client.h>
#include <mpv/render.h>
#include <mpv/stream_cb.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

#ifdef SPARROW_EGL
#include <EGL/egl.h>
#include <EGL/eglext.h>
#define GL_GLEXT_PROTOTYPES
#include <GL/gl.h>
#include <mpv/render_gl.h>
#endif

#define STREAM_URI "sparrow://playback"

typedef struct {
    mpv_handle *player;
    void *stream;
    mpv_stream_cb_read_fn read;
    mpv_stream_cb_cancel_fn cancel;
    atomic_bool opened;
} sparrow_mpv;

static void close_stream(void *cookie) {
    (void)cookie;
}

static int open_stream(void *opaque, char *uri, mpv_stream_cb_info *info) {
    sparrow_mpv *mpv = opaque;
    if (strcmp(uri, STREAM_URI) != 0 || atomic_exchange(&mpv->opened, 1))
        return MPV_ERROR_LOADING_FAILED;
    // One exact Rust byte stream per core. Reopening would resume midstream.
    *info = (mpv_stream_cb_info){
        .cookie = mpv->stream, .read_fn = mpv->read,
        .close_fn = close_stream, .cancel_fn = mpv->cancel,
    };
    return 0;
}

void sparrow_mpv_destroy(sparrow_mpv *mpv) {
    if (mpv->player)
        mpv_terminate_destroy(mpv->player);
    free(mpv);
}

sparrow_mpv *sparrow_mpv_create(void *stream, mpv_stream_cb_read_fn read,
                               mpv_stream_cb_cancel_fn cancel,
                               void *wake_context, void (*wake)(void *),
                               int backend) {
    sparrow_mpv *mpv = calloc(1, sizeof(*mpv));
    if (!mpv)
        return NULL;
    mpv->stream = stream;
    mpv->read = read;
    mpv->cancel = cancel;
    mpv->player = mpv_create();
    atomic_init(&mpv->opened, 0);
    if (!mpv->player)
        goto failed;
    const char *options[][2] = {
        {"config", "no"}, {"terminal", "no"}, {"msg-level", "all=no"},
        {"load-scripts", "no"}, {"autoload-files", "no"}, {"ytdl", "no"},
        {"vo", "libmpv"}, {"hwdec", backend == 0 ? "no" : "auto"}, {"idle", "yes"},
        {"video-timing-offset", "0.012"},
        {"demuxer", "lavf"}, {"demuxer-lavf-format", "mpegts"},
        {"osd-level", "0"}, {"input-default-bindings", "no"},
    };
    for (size_t i = 0; i < sizeof(options) / sizeof(options[0]); ++i) {
        if (mpv_set_option_string(mpv->player, options[i][0], options[i][1]) < 0)
            goto failed;
    }
    if (mpv_initialize(mpv->player) < 0 ||
        mpv_stream_cb_add_ro(mpv->player, "sparrow", mpv, open_stream) < 0)
        goto failed;
    mpv_set_wakeup_callback(mpv->player, wake, wake_context);
    return mpv;

failed:
    sparrow_mpv_destroy(mpv);
    return NULL;
}

typedef struct {
    mpv_render_context *context;
    mpv_handle *player;
#ifdef SPARROW_EGL
    EGLDisplay display;
    EGLContext gl;
    EGLSurface surface;
    GLuint fbo, texture;
    int width, height;
#endif
} sparrow_renderer;

#ifdef SPARROW_EGL
static void *gl_proc(void *unused, const char *name) {
    (void)unused;
    return (void *)eglGetProcAddress(name);
}

static int init_egl(sparrow_renderer *renderer) {
    PFNEGLQUERYDEVICESEXTPROC query_devices =
        (PFNEGLQUERYDEVICESEXTPROC)eglGetProcAddress("eglQueryDevicesEXT");
    PFNEGLQUERYDEVICESTRINGEXTPROC device_string =
        (PFNEGLQUERYDEVICESTRINGEXTPROC)eglGetProcAddress("eglQueryDeviceStringEXT");
    EGLDeviceEXT devices[16];
    EGLint count = 0;
    if (!query_devices || !device_string || !query_devices(16, devices, &count))
        return 0;
    for (EGLint i = 0; i < count; ++i) {
        const char *extensions = device_string(devices[i], EGL_EXTENSIONS);
        if (!extensions || !strstr(extensions, "EGL_EXT_device_drm"))
            continue;
        EGLDisplay display = eglGetPlatformDisplay(EGL_PLATFORM_DEVICE_EXT, devices[i], NULL);
        if (display != EGL_NO_DISPLAY && eglInitialize(display, NULL, NULL)) {
            renderer->display = display;
            break;
        }
    }
    if (renderer->display == EGL_NO_DISPLAY || !eglBindAPI(EGL_OPENGL_API))
        return 0;
    const EGLint config_attributes[] = {
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_BIT,
        EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE,
    };
    EGLConfig config;
    if (!eglChooseConfig(renderer->display, config_attributes, &config, 1, &count) || count == 0)
        return 0;
    const EGLint context_attributes[] = {
        EGL_CONTEXT_MAJOR_VERSION, 3, EGL_CONTEXT_MINOR_VERSION, 3,
        EGL_CONTEXT_OPENGL_PROFILE_MASK, EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT, EGL_NONE,
    };
    renderer->gl = eglCreateContext(renderer->display, config, EGL_NO_CONTEXT, context_attributes);
    const EGLint surface_attributes[] = {EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE};
    renderer->surface = eglCreatePbufferSurface(renderer->display, config, surface_attributes);
    return renderer->gl != EGL_NO_CONTEXT && renderer->surface != EGL_NO_SURFACE &&
           eglMakeCurrent(renderer->display, renderer->surface, renderer->surface, renderer->gl);
}

static void release_egl(sparrow_renderer *renderer) {
    if (renderer->display == EGL_NO_DISPLAY)
        return;
    if (eglGetCurrentContext() == renderer->gl && renderer->gl != EGL_NO_CONTEXT) {
        glDeleteFramebuffers(1, &renderer->fbo);
        glDeleteTextures(1, &renderer->texture);
        eglMakeCurrent(renderer->display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    }
    if (renderer->gl != EGL_NO_CONTEXT)
        eglDestroyContext(renderer->display, renderer->gl);
    if (renderer->surface != EGL_NO_SURFACE)
        eglDestroySurface(renderer->display, renderer->surface);
    eglTerminate(renderer->display);
    eglReleaseThread();
}

static int render_egl(sparrow_renderer *renderer, int width, int height, size_t stride, void *pixels) {
    if (renderer->width != width || renderer->height != height) {
        if (!renderer->texture) {
            glGenFramebuffers(1, &renderer->fbo);
            glGenTextures(1, &renderer->texture);
        }
        glBindTexture(GL_TEXTURE_2D, renderer->texture);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, width, height, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
        glBindFramebuffer(GL_FRAMEBUFFER, renderer->fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, renderer->texture, 0);
        GLenum status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        glBindTexture(GL_TEXTURE_2D, 0);
        if (status != GL_FRAMEBUFFER_COMPLETE)
            return MPV_ERROR_GENERIC;
        renderer->width = width;
        renderer->height = height;
    }
    mpv_opengl_fbo fbo = {renderer->fbo, width, height, GL_RGBA8};
    int block = 0;
    mpv_render_param params[] = {
        {MPV_RENDER_PARAM_OPENGL_FBO, &fbo},
        {MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, &block},
        {0},
    };
    int result = mpv_render_context_render(renderer->context, params);
    if (result < 0)
        return result;
    glBindFramebuffer(GL_READ_FRAMEBUFFER, renderer->fbo);
    glPixelStorei(GL_PACK_ROW_LENGTH, (GLint)(stride / 4));
    glReadPixels(0, 0, width, height, GL_BGRA, GL_UNSIGNED_BYTE, pixels);
    glPixelStorei(GL_PACK_ROW_LENGTH, 0);
    glBindFramebuffer(GL_READ_FRAMEBUFFER, 0);
    return glGetError() == GL_NO_ERROR ? 0 : MPV_ERROR_GENERIC;
}
#endif

void sparrow_mpv_free_render(sparrow_renderer *renderer) {
    if (renderer->context)
        mpv_render_context_free(renderer->context);
#ifdef SPARROW_EGL
    release_egl(renderer);
#endif
    free(renderer);
}

sparrow_renderer *sparrow_mpv_create_render(sparrow_mpv *mpv, void *wake_context,
                                           void (*wake)(void *), int backend) {
    sparrow_renderer *renderer = calloc(1, sizeof(*renderer));
    if (!renderer)
        return NULL;
    mpv_render_param params[] = {
        {MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_API_TYPE_SW}, {0},
    };
    renderer->player = mpv->player;
    int result = MPV_ERROR_UNSUPPORTED;
    if (backend == 0)
        result = mpv_render_context_create(&renderer->context, mpv->player, params);
#ifdef SPARROW_EGL
    else if (init_egl(renderer)) {
        mpv_opengl_init_params gl = {.get_proc_address = gl_proc};
        mpv_render_param gl_params[] = {
            {MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_API_TYPE_OPENGL},
            {MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &gl},
            {0},
        };
        result = mpv_render_context_create(&renderer->context, mpv->player, gl_params);
    }
#endif
    if (result < 0) {
        sparrow_mpv_free_render(renderer);
        return NULL;
    }
    mpv_render_context_set_update_callback(renderer->context, wake, wake_context);
    return renderer;
}

int sparrow_mpv_load(sparrow_mpv *mpv) {
    const char *command[] = {"loadfile", STREAM_URI, NULL};
    return mpv_command(mpv->player, command);
}

int sparrow_mpv_controls(sparrow_mpv *mpv, double volume, int mute, int pause) {
    if (mpv_set_property(mpv->player, "volume", MPV_FORMAT_DOUBLE, &volume) < 0 ||
        mpv_set_property(mpv->player, "mute", MPV_FORMAT_FLAG, &mute) < 0 ||
        mpv_set_property(mpv->player, "pause", MPV_FORMAT_FLAG, &pause) < 0)
        return -1;
    return 0;
}

int sparrow_mpv_event(sparrow_mpv *mpv) {
    mpv_event *event = mpv_wait_event(mpv->player, 0);
    switch (event->event_id) {
    case MPV_EVENT_NONE:
        return 0;
    case MPV_EVENT_END_FILE:
        return ((mpv_event_end_file *)event->data)->reason == MPV_END_FILE_REASON_EOF ? 2 : 3;
    case MPV_EVENT_SHUTDOWN:
    case MPV_EVENT_QUEUE_OVERFLOW:
        return 3;
    default:
        return 1;
    }
}

int sparrow_mpv_update(sparrow_renderer *renderer, int64_t *delay_ns) {
    if (!(mpv_render_context_update(renderer->context) & MPV_RENDER_UPDATE_FRAME))
        return 0;
    mpv_render_frame_info info = {0};
    mpv_render_param param = {MPV_RENDER_PARAM_NEXT_FRAME_INFO, &info};
    if (mpv_render_context_get_info(renderer->context, param) >= 0 &&
        (info.flags & MPV_RENDER_FRAME_INFO_PRESENT) &&
        !(info.flags & (MPV_RENDER_FRAME_INFO_REDRAW | MPV_RENDER_FRAME_INFO_REPEAT))) {
        // mpv 0.41 assigns vo_frame.pts (nanoseconds) despite the header comment.
        *delay_ns = info.target_time ? info.target_time - mpv_get_time_ns(renderer->player) : 0;
        return 2;
    }
    return 1;
}

int sparrow_mpv_render(sparrow_renderer *renderer, int width, int height, size_t stride, void *pixels) {
#ifdef SPARROW_EGL
    if (renderer->display != EGL_NO_DISPLAY)
        return render_egl(renderer, width, height, stride, pixels);
#endif
    int size[] = {width, height};
    int block = 0;
    mpv_render_param params[] = {
        {MPV_RENDER_PARAM_SW_SIZE, size},
        {MPV_RENDER_PARAM_SW_FORMAT, "bgr0"},
        {MPV_RENDER_PARAM_SW_STRIDE, &stride},
        {MPV_RENDER_PARAM_SW_POINTER, pixels},
        {MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, &block},
        {0},
    };
    return mpv_render_context_render(renderer->context, params);
}

int64_t sparrow_mpv_integer(sparrow_mpv *mpv, const char *name) {
    int64_t value = 0;
    return mpv_get_property(mpv->player, name, MPV_FORMAT_INT64, &value) >= 0 ? value : -1;
}

double sparrow_mpv_number(sparrow_mpv *mpv, const char *name) {
    double value = 0;
    return mpv_get_property(mpv->player, name, MPV_FORMAT_DOUBLE, &value) >= 0 ? value : -1;
}

int sparrow_mpv_flag(sparrow_mpv *mpv, const char *name) {
    int value = 0;
    return mpv_get_property(mpv->player, name, MPV_FORMAT_FLAG, &value) >= 0 && value;
}

int sparrow_mpv_string_is(sparrow_mpv *mpv, const char *name, const char *expected) {
    char *value = mpv_get_property_string(mpv->player, name);
    int matches = value && strcmp(value, expected) == 0;
    mpv_free(value);
    return matches;
}

int sparrow_mpv_has_string(sparrow_mpv *mpv, const char *name) {
    char *value = mpv_get_property_string(mpv->player, name);
    int present = value && *value;
    mpv_free(value);
    return present;
}

int sparrow_mpv_private_path(sparrow_mpv *mpv) {
    return sparrow_mpv_string_is(mpv, "path", STREAM_URI) &&
           sparrow_mpv_string_is(mpv, "stream-open-filename", STREAM_URI) &&
           sparrow_mpv_string_is(mpv, "playlist/0/filename", STREAM_URI);
}
