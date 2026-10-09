// NativeActivity del banco con punto de entrada y biblioteca propios: el manifiesto declara
// android.app.lib_name=banconativa (esta biblioteca, libbanconativa.so) y android.app.func_name=banco_nativa_crear.
// La biblioteca NO exporta ANativeActivity_onCreate (ci/build-apk.sh lo comprueba): si la actividad arranca, Android (y,
// en arm64, el traductor) uso el nombre del manifiesto. Corre en su propio proceso (":nativa").
//
// Lineas en el registro (etiqueta "banco"):
//   BANCO nativa_crear=OK|FALLO ...         en el punto de entrada (JNI de la actividad, sdk, rutas)
//   BANCO nativa_ciclo=onStart ...          una por callback de ciclo de vida
//   BANCO nativa=OK|FALLO ...               resumen, en cuanto llegaron onStart, onResume, la ventana, el foco y la cola de
//                                           entrada (la ventana se pinta de verde: 40,200,40)
//   BANCO nativa_entrada=OK tipo=...        primer toque o tecla recibido por la cola de entrada (ALooper)
#include <android/input.h>
#include <android/log.h>
#include <android/looper.h>
#include <android/native_activity.h>
#include <android/native_window.h>
#include <jni.h>
#include <stdint.h>
#include <string.h>

#define TAG "banco"
#define LOG(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)

#if defined(__aarch64__)
#define ABI "arm64"
#elif defined(__x86_64__)
#define ABI "x86_64"
#else
#define ABI "otra"
#endif

// todo ocurre en el hilo principal de la actividad: sin bloqueos
static int crear_ok, inicio, reanudar, ventana, foco, cola, dibujo, resumen, entradas;
static AInputQueue *cola_actual;

static void comprobar(void) {
    if (resumen || !(inicio && reanudar && ventana && foco && cola)) return;
    resumen = 1;
    LOG("BANCO nativa=%s abi=" ABI " crear=%d inicio=%d reanudar=%d ventana=%d foco=%d cola=%d dibujo=%d",
        crear_ok && dibujo ? "OK" : "FALLO", crear_ok, inicio, reanudar, ventana, foco, cola, dibujo);
}

/** Pinta toda la ventana de verde (40,200,40). Devuelve 1 si pudo. */
static int pintar(ANativeWindow *w) {
    if (!w) return 0;
    ANativeWindow_setBuffersGeometry(w, 0, 0, WINDOW_FORMAT_RGBX_8888);
    ANativeWindow_Buffer b;
    if (ANativeWindow_lock(w, &b, NULL) != 0) return 0;
    int ok = b.format == WINDOW_FORMAT_RGBX_8888 || b.format == WINDOW_FORMAT_RGBA_8888;
    if (ok) {
        // RGBX en memoria: bytes R, G, B, X
        for (int y = 0; y < b.height; y++) {
            uint32_t *fila = (uint32_t *)b.bits + (size_t)y * b.stride;
            for (int x = 0; x < b.width; x++) fila[x] = 0xff28c828u;
        }
    }
    ANativeWindow_unlockAndPost(w);
    LOG("BANCO nativa_ciclo=dibujo tam=%dx%d formato=%d ok=%d", b.width, b.height, b.format, ok);
    return ok;
}

static int on_input(int fd, int events, void *data) {
    (void)fd;
    (void)events;
    AInputQueue *q = data;
    AInputEvent *e = NULL;
    while (AInputQueue_getEvent(q, &e) >= 0) {
        if (AInputQueue_preDispatchEvent(q, e)) continue;
        int tipo = AInputEvent_getType(e), fuente = AInputEvent_getSource(e);
        if (tipo == AINPUT_EVENT_TYPE_MOTION && (AMotionEvent_getAction(e) & AMOTION_EVENT_ACTION_MASK) == AMOTION_EVENT_ACTION_DOWN && entradas < 8) {
            entradas++;
            LOG("BANCO nativa_entrada=OK tipo=toque x=%.0f y=%.0f fuente=%s", AMotionEvent_getX(e, 0), AMotionEvent_getY(e, 0),
                (fuente & AINPUT_SOURCE_TOUCHSCREEN) == AINPUT_SOURCE_TOUCHSCREEN ? "tactil" : "otra");
        } else if (tipo == AINPUT_EVENT_TYPE_KEY && AKeyEvent_getAction(e) == AKEY_EVENT_ACTION_DOWN && entradas < 8) {
            entradas++;
            LOG("BANCO nativa_entrada=OK tipo=tecla codigo=%d", AKeyEvent_getKeyCode(e));
        }
        AInputQueue_finishEvent(q, e, 1);
    }
    return 1;
}

static void on_start(ANativeActivity *a) {
    (void)a;
    inicio = 1;
    LOG("BANCO nativa_ciclo=onStart");
    comprobar();
}

static void on_resume(ANativeActivity *a) {
    (void)a;
    reanudar = 1;
    LOG("BANCO nativa_ciclo=onResume");
    comprobar();
}

static void on_pause(ANativeActivity *a) {
    (void)a;
    LOG("BANCO nativa_ciclo=onPause");
}

static void on_stop(ANativeActivity *a) {
    (void)a;
    LOG("BANCO nativa_ciclo=onStop");
}

static void on_destroy(ANativeActivity *a) {
    (void)a;
    LOG("BANCO nativa_ciclo=onDestroy");
}

static void on_focus(ANativeActivity *a, int f) {
    (void)a;
    LOG("BANCO nativa_ciclo=onWindowFocusChanged foco=%d", f);
    if (f) foco = 1;
    comprobar();
}

static void on_window(ANativeActivity *a, ANativeWindow *w) {
    (void)a;
    ventana = 1;
    LOG("BANCO nativa_ciclo=onNativeWindowCreated");
    dibujo = pintar(w);
    comprobar();
}

static void on_redraw(ANativeActivity *a, ANativeWindow *w) {
    (void)a;
    pintar(w);
}

static void on_window_destroyed(ANativeActivity *a, ANativeWindow *w) {
    (void)a;
    (void)w;
    LOG("BANCO nativa_ciclo=onNativeWindowDestroyed");
}

static void on_queue(ANativeActivity *a, AInputQueue *q) {
    (void)a;
    LOG("BANCO nativa_ciclo=onInputQueueCreated");
    ALooper *l = ALooper_forThread();
    if (l && q) {
        AInputQueue_attachLooper(q, l, 1, on_input, q);
        cola_actual = q;
        cola = 1;
    }
    comprobar();
}

static void on_queue_destroyed(ANativeActivity *a, AInputQueue *q) {
    (void)a;
    LOG("BANCO nativa_ciclo=onInputQueueDestroyed");
    if (q && q == cola_actual) {
        AInputQueue_detachLooper(q);
        cola_actual = NULL;
    }
}

/** Punto de entrada que declara el manifiesto (android.app.func_name), con la firma de ANativeActivity_onCreate. */
__attribute__((visibility("default"))) void banco_nativa_crear(ANativeActivity *a, void *estado, size_t tam) {
    // el JNIEnv de la actividad: version y nombre del paquete por la propia actividad (clazz)
    JNIEnv *env = a ? a->env : NULL;
    jint ver = env ? (*env)->GetVersion(env) : 0;
    char paquete[64] = "?";
    if (env && a->clazz) {
        jclass c = (*env)->GetObjectClass(env, a->clazz);
        jmethodID m = c ? (*env)->GetMethodID(env, c, "getPackageName", "()Ljava/lang/String;") : NULL;
        jstring s = m ? (jstring)(*env)->CallObjectMethod(env, a->clazz, m) : NULL;
        if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
        const char *p = s ? (*env)->GetStringUTFChars(env, s, NULL) : NULL;
        if (p) {
            strncpy(paquete, p, sizeof paquete - 1);
            (*env)->ReleaseStringUTFChars(env, s, p);
        }
        if (s) (*env)->DeleteLocalRef(env, s);
        if (c) (*env)->DeleteLocalRef(env, c);
    }
    crear_ok = a && a->callbacks && a->vm && ver >= JNI_VERSION_1_6 && strcmp(paquete, "rs.weft.banco") == 0 && a->sdkVersion >= 24 &&
               a->internalDataPath && a->assetManager;
    LOG("BANCO nativa_crear=%s func=banco_nativa_crear lib=banconativa abi=" ABI " jni=0x%x paquete=%s sdk=%d datos=%s estado=%zu",
        crear_ok ? "OK" : "FALLO", (unsigned)ver, paquete, a ? (int)a->sdkVersion : -1, a && a->internalDataPath ? "si" : "no",
        estado ? tam : (size_t)0);
    if (!a || !a->callbacks) return;
    ANativeActivityCallbacks *cb = a->callbacks;
    cb->onStart = on_start;
    cb->onResume = on_resume;
    cb->onPause = on_pause;
    cb->onStop = on_stop;
    cb->onDestroy = on_destroy;
    cb->onWindowFocusChanged = on_focus;
    cb->onNativeWindowCreated = on_window;
    cb->onNativeWindowRedrawNeeded = on_redraw;
    cb->onNativeWindowDestroyed = on_window_destroyed;
    cb->onInputQueueCreated = on_queue;
    cb->onInputQueueDestroyed = on_queue_destroyed;
}
