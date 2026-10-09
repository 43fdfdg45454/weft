// Biblioteca nativa del banco de pruebas. Se compila para una sola arquitectura por paquete.
// Cada funcion devuelve "OK detalle" o "FALLO detalle".
#include <android/log.h>
#include <dlfcn.h>
#include <jni.h>
#include "bench.h"
#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <EGL/egl.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>
#include <GLES2/gl2.h>
#include <vulkan/vulkan.h>

#define FN(name) Java_rs_weft_banco_Main_##name
static jstring str(JNIEnv *env, const char *s) { return (*env)->NewStringUTF(env, s); }

JNIEXPORT jstring JNICALL FN(abi)(JNIEnv *env, jclass c) {
    (void)c;
#if defined(__aarch64__)
    return str(env, "abi=arm64");
#elif defined(__x86_64__)
    return str(env, "abi=x86_64");
#else
    return str(env, "abi=otra");
#endif
}

// ------------------------------------------------------------------------------------------------ calculo
JNIEXPORT jstring JNICALL FN(calculo)(JNIEnv *env, jclass c) {
    (void)c;
    char out[200];
    // enteros
    uint64_t x = 0;
    for (uint64_t i = 1; i <= 100000; i++) x = (x * 31 + i * i) % 1000000007ULL;
    // coma flotante
    double s = 0;
    for (int i = 0; i < 100000; i++) s += sin(i * 0.001);
    // memoria y cadenas
    size_t n = 1 << 20;
    unsigned char *a = malloc(n), *b = malloc(n);
    for (size_t i = 0; i < n; i++) a[i] = (unsigned char)(i * 7 + 3);
    memcpy(b, a, n);
    int mem_ok = memcmp(a, b, n) == 0;
    free(a);
    free(b);
    char t[64];
    snprintf(t, sizeof t, "%d-%s-%.2f", 42, "texto", 3.14159);
    int str_ok = strcmp(t, "42-texto-3.14") == 0;
    int ok = x == 993573784ULL && fabs(s - 137.934299) < 1e-4 && mem_ok && str_ok;
    snprintf(out, sizeof out, "%s entero=%llu seno=%.4f memoria=%d texto=%d", ok ? "OK" : "FALLO", (unsigned long long)x, s, mem_ok, str_ok);
    return str(env, out);
}

static atomic_long counter;
static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static long guarded;
static void *worker(void *arg) {
    (void)arg;
    for (int i = 0; i < 100000; i++) {
        atomic_fetch_add(&counter, 1);
        pthread_mutex_lock(&mu);
        guarded++;
        pthread_mutex_unlock(&mu);
    }
    return NULL;
}

JNIEXPORT jstring JNICALL FN(hilos)(JNIEnv *env, jclass c) {
    (void)c;
    char out[120];
    pthread_t th[4];
    atomic_store(&counter, 0);
    guarded = 0;
    for (int i = 0; i < 4; i++) pthread_create(&th[i], NULL, worker, NULL);
    for (int i = 0; i < 4; i++) pthread_join(th[i], NULL);
    long a = atomic_load(&counter);
    snprintf(out, sizeof out, "%s atomico=%ld con_cerrojo=%ld", (a == 400000 && guarded == 400000) ? "OK" : "FALLO", a, guarded);
    return str(env, out);
}

// ------------------------------------------------------------------------------------------------ OpenGL ES 2
static GLuint shader(GLenum type, const char *src) {
    GLuint s = glCreateShader(type);
    glShaderSource(s, 1, &src, NULL);
    glCompileShader(s);
    GLint ok = 0;
    glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    return ok ? s : 0;
}

JNIEXPORT jstring JNICALL FN(opengl)(JNIEnv *env, jclass c) {
    (void)c;
    char out[300];
    EGLDisplay d = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (d == EGL_NO_DISPLAY || !eglInitialize(d, NULL, NULL)) return str(env, "FALLO eglInitialize");
    EGLint cfg_attr[] = {EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE};
    EGLConfig cfg;
    EGLint n = 0;
    if (!eglChooseConfig(d, cfg_attr, &cfg, 1, &n) || n < 1) return str(env, "FALLO eglChooseConfig");
    EGLint pb_attr[] = {EGL_WIDTH, 64, EGL_HEIGHT, 64, EGL_NONE};
    EGLSurface surf = eglCreatePbufferSurface(d, cfg, pb_attr);
    EGLint ctx_attr[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    EGLContext ctx = eglCreateContext(d, cfg, EGL_NO_CONTEXT, ctx_attr);
    if (surf == EGL_NO_SURFACE || ctx == EGL_NO_CONTEXT || !eglMakeCurrent(d, surf, surf, ctx)) return str(env, "FALLO contexto EGL");
    const char *vs = "attribute vec2 p; void main(){ gl_Position = vec4(p, 0.0, 1.0); }";
    const char *fs = "precision mediump float; uniform vec4 col; void main(){ gl_FragColor = col; }";
    GLuint v = shader(GL_VERTEX_SHADER, vs), f = shader(GL_FRAGMENT_SHADER, fs);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, v);
    glAttachShader(prog, f);
    glBindAttribLocation(prog, 0, "p");
    glLinkProgram(prog);
    GLint linked = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &linked);
    if (!v || !f || !linked) {
        eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        return str(env, "FALLO sombreadores");
    }
    glViewport(0, 0, 64, 64);
    glClearColor(0, 0, 0, 1);
    glClear(GL_COLOR_BUFFER_BIT);
    glUseProgram(prog);
    glUniform4f(glGetUniformLocation(prog, "col"), 0.25f, 0.5f, 0.75f, 1.0f);
    // un triangulo que cubre la mitad inferior izquierda
    static const GLfloat tri[] = {-1, -1, 1, -1, -1, 1};
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, tri);
    glEnableVertexAttribArray(0);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    unsigned char in[4] = {0}, outp[4] = {0};
    glReadPixels(10, 10, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, in);    // dentro del triangulo
    glReadPixels(60, 60, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, outp);  // fuera
    const char *renderer = (const char *)glGetString(GL_RENDERER);
    const char *version = (const char *)glGetString(GL_VERSION);
    int ok = abs(in[0] - 64) <= 2 && abs(in[1] - 128) <= 2 && abs(in[2] - 191) <= 2 && outp[0] == 0 && outp[1] == 0 && outp[2] == 0 && glGetError() == GL_NO_ERROR;
    snprintf(out, sizeof out, "%s dentro=%d,%d,%d fuera=%d,%d,%d motor=[%.60s] version=[%.40s]", ok ? "OK" : "FALLO", in[0], in[1], in[2], outp[0], outp[1], outp[2], renderer ? renderer : "?", version ? version : "?");
    eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(d, ctx);
    eglDestroySurface(d, surf);
    return str(env, out);
}

// ------------------------------------------------------------------------------------------------ Vulkan
// Como lo hace una aplicacion (o volk): la biblioteca no enlaza libvulkan.so; se abre con dlopen, de ella solo se toma
// vkGetInstanceProcAddr y todo lo demas se pide por vkGetInstanceProcAddr (globales e instancia) y vkGetDeviceProcAddr
// (dispositivo). Con un traductor de ARM esto pasa por los punteros a funcion que devuelve el anfitrion.
#define VK_GLOBAL(X) X(CreateInstance)
#define VK_INST(X) \
    X(DestroyInstance) X(EnumeratePhysicalDevices) X(GetPhysicalDeviceProperties) X(GetPhysicalDeviceQueueFamilyProperties) \
    X(GetPhysicalDeviceMemoryProperties) X(CreateDevice) X(GetDeviceProcAddr)
#define VK_DEV(X) \
    X(DestroyDevice) X(GetDeviceQueue) X(CreateImage) X(DestroyImage) X(GetImageMemoryRequirements) X(AllocateMemory) X(FreeMemory) \
    X(BindImageMemory) X(CreateImageView) X(DestroyImageView) X(CreateRenderPass) X(DestroyRenderPass) X(CreateFramebuffer) \
    X(DestroyFramebuffer) X(CreateBuffer) X(DestroyBuffer) X(GetBufferMemoryRequirements) X(BindBufferMemory) X(CreateShaderModule) \
    X(DestroyShaderModule) X(CreatePipelineLayout) X(DestroyPipelineLayout) X(CreateGraphicsPipelines) X(DestroyPipeline) \
    X(CreateCommandPool) X(DestroyCommandPool) X(AllocateCommandBuffers) X(BeginCommandBuffer) X(EndCommandBuffer) \
    X(CmdBeginRenderPass) X(CmdEndRenderPass) X(CmdBindPipeline) X(CmdDraw) X(CmdPipelineBarrier) X(CmdCopyImageToBuffer) \
    X(CreateFence) X(DestroyFence) X(WaitForFences) X(QueueSubmit) X(MapMemory) X(UnmapMemory)
static struct {
    PFN_vkGetInstanceProcAddr GetInstanceProcAddr;
#define X(n) PFN_vk##n n;
    VK_GLOBAL(X) VK_INST(X) VK_DEV(X)
#undef X
} vk;

// Sombreadores SPIR-V 1.0 escritos a mano (validados con spirv-val --target-env vulkan1.0 del NDK). Equivalen a:
//   vertices: void main() { gl_Position = vec4(gl_VertexIndex == 1 ? 1.0 : -1.0, gl_VertexIndex == 2 ? 1.0 : -1.0, 0.0, 1.0); }
//   fragmentos: layout(location = 0) out vec4 color; void main() { color = vec4(1.0, 0.5, 0.25, 1.0); }
// Tres vertices, sin buferes: el triangulo (-1,-1) (1,-1) (-1,1) cubre la mitad superior izquierda de la imagen (x + y < 64).
static const uint32_t spv_vs[] = {
    0x07230203, 0x00010000, 0x00000000, 0x0000001c, 0x00000000, 0x00020011, 0x00000001, 0x0003000e,
    0x00000000, 0x00000001, 0x0007000f, 0x00000000, 0x00000001, 0x6e69616d, 0x00000000, 0x00000002,
    0x00000003, 0x00040047, 0x00000002, 0x0000000b, 0x0000002a, 0x00050048, 0x00000004, 0x00000000,
    0x0000000b, 0x00000000, 0x00030047, 0x00000004, 0x00000002, 0x00020013, 0x00000005, 0x00030021,
    0x00000006, 0x00000005, 0x00030016, 0x00000007, 0x00000020, 0x00040015, 0x00000008, 0x00000020,
    0x00000001, 0x00040017, 0x00000009, 0x00000007, 0x00000004, 0x00020014, 0x0000000a, 0x0003001e,
    0x00000004, 0x00000009, 0x00040020, 0x0000000b, 0x00000003, 0x00000004, 0x00040020, 0x0000000c,
    0x00000001, 0x00000008, 0x00040020, 0x0000000d, 0x00000003, 0x00000009, 0x0004003b, 0x0000000b,
    0x00000003, 0x00000003, 0x0004003b, 0x0000000c, 0x00000002, 0x00000001, 0x0004002b, 0x00000008,
    0x0000000e, 0x00000000, 0x0004002b, 0x00000008, 0x0000000f, 0x00000001, 0x0004002b, 0x00000008,
    0x00000010, 0x00000002, 0x0004002b, 0x00000007, 0x00000011, 0x3f800000, 0x0004002b, 0x00000007,
    0x00000012, 0xbf800000, 0x0004002b, 0x00000007, 0x00000013, 0x00000000, 0x00050036, 0x00000005,
    0x00000001, 0x00000000, 0x00000006, 0x000200f8, 0x00000014, 0x0004003d, 0x00000008, 0x00000015,
    0x00000002, 0x000500aa, 0x0000000a, 0x00000016, 0x00000015, 0x0000000f, 0x000500aa, 0x0000000a,
    0x00000017, 0x00000015, 0x00000010, 0x000600a9, 0x00000007, 0x00000018, 0x00000016, 0x00000011,
    0x00000012, 0x000600a9, 0x00000007, 0x00000019, 0x00000017, 0x00000011, 0x00000012, 0x00070050,
    0x00000009, 0x0000001a, 0x00000018, 0x00000019, 0x00000013, 0x00000011, 0x00050041, 0x0000000d,
    0x0000001b, 0x00000003, 0x0000000e, 0x0003003e, 0x0000001b, 0x0000001a, 0x000100fd, 0x00010038,
};
static const uint32_t spv_fs[] = {
    0x07230203, 0x00010000, 0x00000000, 0x0000000d, 0x00000000, 0x00020011, 0x00000001, 0x0003000e,
    0x00000000, 0x00000001, 0x0006000f, 0x00000004, 0x00000001, 0x6e69616d, 0x00000000, 0x00000002,
    0x00030010, 0x00000001, 0x00000007, 0x00040047, 0x00000002, 0x0000001e, 0x00000000, 0x00020013,
    0x00000003, 0x00030021, 0x00000004, 0x00000003, 0x00030016, 0x00000005, 0x00000020, 0x00040017,
    0x00000006, 0x00000005, 0x00000004, 0x00040020, 0x00000007, 0x00000003, 0x00000006, 0x0004003b,
    0x00000007, 0x00000002, 0x00000003, 0x0004002b, 0x00000005, 0x00000008, 0x3f800000, 0x0004002b,
    0x00000005, 0x00000009, 0x3f000000, 0x0004002b, 0x00000005, 0x0000000a, 0x3e800000, 0x0007002c,
    0x00000006, 0x0000000b, 0x00000008, 0x00000009, 0x0000000a, 0x00000008, 0x00050036, 0x00000003,
    0x00000001, 0x00000000, 0x00000004, 0x000200f8, 0x0000000c, 0x0003003e, 0x00000002, 0x0000000b,
    0x000100fd, 0x00010038,
};

#define VK_LADO 64
// Todo lo creado; vk_destruir suelta lo que no sea nulo, en orden inverso, desde cualquier punto del caso.
typedef struct {
    VkInstance inst;
    VkDevice dev;
    VkImage img;
    VkDeviceMemory imem, bmem;
    VkImageView view;
    VkRenderPass rp;
    VkFramebuffer fb;
    VkBuffer buf;
    VkShaderModule vs, fs;
    VkPipelineLayout layout;
    VkPipeline pipe;
    VkCommandPool pool;
    VkFence fence;
} VkCaso;

static void vk_destruir(VkCaso *k) {
    if (k->dev) {
        if (k->fence) vk.DestroyFence(k->dev, k->fence, NULL);
        if (k->pool) vk.DestroyCommandPool(k->dev, k->pool, NULL);  // libera tambien el bufer de ordenes
        if (k->pipe) vk.DestroyPipeline(k->dev, k->pipe, NULL);
        if (k->layout) vk.DestroyPipelineLayout(k->dev, k->layout, NULL);
        if (k->fs) vk.DestroyShaderModule(k->dev, k->fs, NULL);
        if (k->vs) vk.DestroyShaderModule(k->dev, k->vs, NULL);
        if (k->buf) vk.DestroyBuffer(k->dev, k->buf, NULL);
        if (k->bmem) vk.FreeMemory(k->dev, k->bmem, NULL);
        if (k->fb) vk.DestroyFramebuffer(k->dev, k->fb, NULL);
        if (k->rp) vk.DestroyRenderPass(k->dev, k->rp, NULL);
        if (k->view) vk.DestroyImageView(k->dev, k->view, NULL);
        if (k->img) vk.DestroyImage(k->dev, k->img, NULL);
        if (k->imem) vk.FreeMemory(k->dev, k->imem, NULL);
        if (vk.DestroyDevice) vk.DestroyDevice(k->dev, NULL);
    }
    if (k->inst && vk.DestroyInstance) vk.DestroyInstance(k->inst, NULL);
    memset(k, 0, sizeof *k);
}

// Primer tipo de memoria admitido por `bits` que tenga las propiedades `quiero` (UINT32_MAX si ninguno).
static uint32_t vk_tipo_memoria(const VkPhysicalDeviceMemoryProperties *mp, uint32_t bits, VkMemoryPropertyFlags quiero) {
    for (uint32_t i = 0; i < mp->memoryTypeCount; i++)
        if ((bits & (1u << i)) && (mp->memoryTypes[i].propertyFlags & quiero) == quiero) return i;
    return UINT32_MAX;
}

// Dibuja fuera de pantalla con la GPU: borra una imagen de 64x64 a un color, pinta encima un triangulo con los
// sombreadores de arriba, copia la imagen a un bufer visible desde la CPU y comprueba un pixel dentro del triangulo y
// otro fuera. Sin ningun dispositivo Vulkan (o sin controlador: VK_ERROR_INCOMPATIBLE_DRIVER al crear la instancia) el
// resultado es "OMITIDO motivo=sin_vulkan": no es un fallo de la aplicacion ni del traductor.
static void vk_caso(char *out, size_t cap) {
    VkCaso k = {0};
    VkResult r = VK_SUCCESS;
#define VKMAL(que) do { snprintf(out, cap, "FALLO %s (%d)", que, (int)r); vk_destruir(&k); return; } while (0)
#define VKCOMPRUEBA(llamada, que) do { if ((r = (llamada)) != VK_SUCCESS) VKMAL(que); } while (0)
    static void *lib;
    if (!lib && !(lib = dlopen("libvulkan.so", RTLD_NOW | RTLD_LOCAL))) {
        const char *e = dlerror();
        snprintf(out, cap, "FALLO sin libvulkan.so (dlopen: %.120s)", e ? e : "?");
        return;
    }
    if (!(vk.GetInstanceProcAddr = (PFN_vkGetInstanceProcAddr)dlsym(lib, "vkGetInstanceProcAddr"))) {
        snprintf(out, cap, "FALLO libvulkan.so sin vkGetInstanceProcAddr");
        return;
    }
#define X(n) if (!(vk.n = (PFN_vk##n)vk.GetInstanceProcAddr(NULL, "vk" #n))) { snprintf(out, cap, "FALLO vkGetInstanceProcAddr(NULL, vk%s) = NULL", #n); return; }
    VK_GLOBAL(X)
#undef X

    VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .pApplicationName = "banco", .apiVersion = VK_API_VERSION_1_0};
    VkInstanceCreateInfo ici = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app};
    r = vk.CreateInstance(&ici, NULL, &k.inst);
    if (r == VK_ERROR_INCOMPATIBLE_DRIVER) {
        snprintf(out, cap, "OMITIDO motivo=sin_vulkan vkCreateInstance=%d", (int)r);
        return;
    }
    if (r != VK_SUCCESS) { k.inst = VK_NULL_HANDLE; VKMAL("vkCreateInstance"); }
#define X(n) if (!(vk.n = (PFN_vk##n)vk.GetInstanceProcAddr(k.inst, "vk" #n))) { r = -1; VKMAL("vkGetInstanceProcAddr(vk" #n ") = NULL"); }
    VK_INST(X)
#undef X
    // primero cuantos hay: sin ninguno no hay nada que probar
    uint32_t n = 0;
    VKCOMPRUEBA(vk.EnumeratePhysicalDevices(k.inst, &n, NULL), "vkEnumeratePhysicalDevices");
    if (n == 0) {
        snprintf(out, cap, "OMITIDO motivo=sin_vulkan dispositivos=0");
        vk_destruir(&k);
        return;
    }
    VkPhysicalDevice fisicos[8];
    uint32_t total = n;
    if (n > 8) n = 8;
    r = vk.EnumeratePhysicalDevices(k.inst, &n, fisicos);
    if ((r != VK_SUCCESS && r != VK_INCOMPLETE) || n == 0) VKMAL("vkEnumeratePhysicalDevices (lista)");
    // el primer dispositivo con una cola grafica
    VkPhysicalDevice phys = VK_NULL_HANDLE;
    uint32_t qi = UINT32_MAX;
    for (uint32_t d = 0; d < n && qi == UINT32_MAX; d++) {
        VkQueueFamilyProperties qf[16];
        uint32_t qn = 16;
        vk.GetPhysicalDeviceQueueFamilyProperties(fisicos[d], &qn, qf);
        for (uint32_t i = 0; i < qn && i < 16; i++)
            if (qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) { phys = fisicos[d]; qi = i; break; }
    }
    if (qi == UINT32_MAX) { r = -1; VKMAL("ningun dispositivo con cola grafica"); }
    VkPhysicalDeviceProperties props;
    vk.GetPhysicalDeviceProperties(phys, &props);
    VkPhysicalDeviceMemoryProperties mp;
    vk.GetPhysicalDeviceMemoryProperties(phys, &mp);

    float prio = 1.0f;
    VkDeviceQueueCreateInfo qci = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, .queueFamilyIndex = qi, .queueCount = 1, .pQueuePriorities = &prio};
    VkDeviceCreateInfo dci = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, .queueCreateInfoCount = 1, .pQueueCreateInfos = &qci};
    VKCOMPRUEBA(vk.CreateDevice(phys, &dci, NULL, &k.dev), "vkCreateDevice");
#define X(n) if (!(vk.n = (PFN_vk##n)vk.GetDeviceProcAddr(k.dev, "vk" #n))) { r = -1; VKMAL("vkGetDeviceProcAddr(vk" #n ") = NULL"); }
    VK_DEV(X)
#undef X
    VkQueue queue = VK_NULL_HANDLE;
    vk.GetDeviceQueue(k.dev, qi, 0, &queue);
    if (!queue) { r = -1; VKMAL("vkGetDeviceQueue"); }

    // imagen de color en memoria del dispositivo (si la hay)
    VkImageCreateInfo imci = {.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO, .imageType = VK_IMAGE_TYPE_2D, .format = VK_FORMAT_R8G8B8A8_UNORM,
        .extent = {VK_LADO, VK_LADO, 1}, .mipLevels = 1, .arrayLayers = 1, .samples = VK_SAMPLE_COUNT_1_BIT, .tiling = VK_IMAGE_TILING_OPTIMAL,
        .usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT, .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED};
    VKCOMPRUEBA(vk.CreateImage(k.dev, &imci, NULL, &k.img), "vkCreateImage");
    VkMemoryRequirements mr;
    vk.GetImageMemoryRequirements(k.dev, k.img, &mr);
    uint32_t mt = vk_tipo_memoria(&mp, mr.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    if (mt == UINT32_MAX) mt = vk_tipo_memoria(&mp, mr.memoryTypeBits, 0);
    if (mt == UINT32_MAX) { r = -1; VKMAL("sin tipo de memoria para la imagen"); }
    VkMemoryAllocateInfo mai = {.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = mr.size, .memoryTypeIndex = mt};
    VKCOMPRUEBA(vk.AllocateMemory(k.dev, &mai, NULL, &k.imem), "memoria de imagen");
    VKCOMPRUEBA(vk.BindImageMemory(k.dev, k.img, k.imem, 0), "vkBindImageMemory");
    VkImageViewCreateInfo ivci = {.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO, .image = k.img, .viewType = VK_IMAGE_VIEW_TYPE_2D,
        .format = VK_FORMAT_R8G8B8A8_UNORM, .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
    VKCOMPRUEBA(vk.CreateImageView(k.dev, &ivci, NULL, &k.view), "vkCreateImageView");

    // pase de dibujo: borra al color de fondo y deja la imagen lista para copiarla; la dependencia de salida ordena la
    // escritura del color antes de la copia
    VkAttachmentDescription att = {.format = VK_FORMAT_R8G8B8A8_UNORM, .samples = VK_SAMPLE_COUNT_1_BIT, .loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR,
        .storeOp = VK_ATTACHMENT_STORE_OP_STORE, .stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE, .stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE,
        .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED, .finalLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL};
    VkAttachmentReference ref = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
    VkSubpassDescription sub = {.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS, .colorAttachmentCount = 1, .pColorAttachments = &ref};
    VkSubpassDependency dep = {.srcSubpass = 0, .dstSubpass = VK_SUBPASS_EXTERNAL, .srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
        .dstStageMask = VK_PIPELINE_STAGE_TRANSFER_BIT, .srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT, .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT};
    VkRenderPassCreateInfo rpci = {.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO, .attachmentCount = 1, .pAttachments = &att, .subpassCount = 1,
        .pSubpasses = &sub, .dependencyCount = 1, .pDependencies = &dep};
    VKCOMPRUEBA(vk.CreateRenderPass(k.dev, &rpci, NULL, &k.rp), "vkCreateRenderPass");
    VkFramebufferCreateInfo fbci = {.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO, .renderPass = k.rp, .attachmentCount = 1, .pAttachments = &k.view,
        .width = VK_LADO, .height = VK_LADO, .layers = 1};
    VKCOMPRUEBA(vk.CreateFramebuffer(k.dev, &fbci, NULL, &k.fb), "vkCreateFramebuffer");

    // tuberia grafica: los dos sombreadores, sin entrada de vertices, sin profundidad ni mezcla
    VkShaderModuleCreateInfo smci = {.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, .codeSize = sizeof spv_vs, .pCode = spv_vs};
    VKCOMPRUEBA(vk.CreateShaderModule(k.dev, &smci, NULL, &k.vs), "vkCreateShaderModule (vertices)");
    smci.codeSize = sizeof spv_fs;
    smci.pCode = spv_fs;
    VKCOMPRUEBA(vk.CreateShaderModule(k.dev, &smci, NULL, &k.fs), "vkCreateShaderModule (fragmentos)");
    VkPipelineLayoutCreateInfo plci = {.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    VKCOMPRUEBA(vk.CreatePipelineLayout(k.dev, &plci, NULL, &k.layout), "vkCreatePipelineLayout");
    VkPipelineShaderStageCreateInfo etapas[2] = {
        {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, .stage = VK_SHADER_STAGE_VERTEX_BIT, .module = k.vs, .pName = "main"},
        {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, .stage = VK_SHADER_STAGE_FRAGMENT_BIT, .module = k.fs, .pName = "main"},
    };
    VkPipelineVertexInputStateCreateInfo vin = {.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo ia = {.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO, .topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST};
    VkViewport vp = {0, 0, VK_LADO, VK_LADO, 0, 1};
    VkRect2D sc = {{0, 0}, {VK_LADO, VK_LADO}};
    VkPipelineViewportStateCreateInfo vps = {.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO, .viewportCount = 1, .pViewports = &vp,
        .scissorCount = 1, .pScissors = &sc};
    VkPipelineRasterizationStateCreateInfo ras = {.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO, .polygonMode = VK_POLYGON_MODE_FILL,
        .cullMode = VK_CULL_MODE_NONE, .frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE, .lineWidth = 1.0f};
    VkPipelineMultisampleStateCreateInfo ms = {.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO, .rasterizationSamples = VK_SAMPLE_COUNT_1_BIT};
    VkPipelineColorBlendAttachmentState cba = {.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT | VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT};
    VkPipelineColorBlendStateCreateInfo cb = {.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO, .attachmentCount = 1, .pAttachments = &cba};
    VkGraphicsPipelineCreateInfo gpci = {.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO, .stageCount = 2, .pStages = etapas, .pVertexInputState = &vin,
        .pInputAssemblyState = &ia, .pViewportState = &vps, .pRasterizationState = &ras, .pMultisampleState = &ms, .pColorBlendState = &cb,
        .layout = k.layout, .renderPass = k.rp, .subpass = 0};
    VKCOMPRUEBA(vk.CreateGraphicsPipelines(k.dev, VK_NULL_HANDLE, 1, &gpci, NULL, &k.pipe), "vkCreateGraphicsPipelines");

    // bufer de lectura, visible y coherente desde la CPU
    VkBufferCreateInfo bci = {.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, .size = VK_LADO * VK_LADO * 4, .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT};
    VKCOMPRUEBA(vk.CreateBuffer(k.dev, &bci, NULL, &k.buf), "vkCreateBuffer");
    vk.GetBufferMemoryRequirements(k.dev, k.buf, &mr);
    mt = vk_tipo_memoria(&mp, mr.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    if (mt == UINT32_MAX) { r = -1; VKMAL("sin memoria visible y coherente"); }
    mai.allocationSize = mr.size;
    mai.memoryTypeIndex = mt;
    VKCOMPRUEBA(vk.AllocateMemory(k.dev, &mai, NULL, &k.bmem), "memoria de bufer");
    VKCOMPRUEBA(vk.BindBufferMemory(k.dev, k.buf, k.bmem, 0), "vkBindBufferMemory");

    // ordenes: borrar, triangulo, copiar a la memoria visible y hacerla legible por la CPU
    VkCommandPoolCreateInfo cpci = {.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, .queueFamilyIndex = qi};
    VKCOMPRUEBA(vk.CreateCommandPool(k.dev, &cpci, NULL, &k.pool), "vkCreateCommandPool");
    VkCommandBufferAllocateInfo cbai = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, .commandPool = k.pool, .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
        .commandBufferCount = 1};
    VkCommandBuffer cmd;
    VKCOMPRUEBA(vk.AllocateCommandBuffers(k.dev, &cbai, &cmd), "vkAllocateCommandBuffers");
    VkCommandBufferBeginInfo cbbi = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO, .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
    VKCOMPRUEBA(vk.BeginCommandBuffer(cmd, &cbbi), "vkBeginCommandBuffer");
    VkClearValue fondo = {.color = {.float32 = {0.25f, 0.5f, 0.75f, 1.0f}}};
    VkRenderPassBeginInfo rpbi = {.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO, .renderPass = k.rp, .framebuffer = k.fb, .renderArea = sc,
        .clearValueCount = 1, .pClearValues = &fondo};
    vk.CmdBeginRenderPass(cmd, &rpbi, VK_SUBPASS_CONTENTS_INLINE);
    vk.CmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, k.pipe);
    vk.CmdDraw(cmd, 3, 1, 0, 0);
    vk.CmdEndRenderPass(cmd);
    VkBufferImageCopy region = {.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}, .imageExtent = {VK_LADO, VK_LADO, 1}};
    vk.CmdCopyImageToBuffer(cmd, k.img, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, k.buf, 1, &region);
    VkBufferMemoryBarrier bb = {.sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER, .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT, .dstAccessMask = VK_ACCESS_HOST_READ_BIT,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .buffer = k.buf, .offset = 0, .size = VK_WHOLE_SIZE};
    vk.CmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT, 0, 0, NULL, 1, &bb, 0, NULL);
    VKCOMPRUEBA(vk.EndCommandBuffer(cmd), "vkEndCommandBuffer");
    VkFenceCreateInfo fci = {.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    VKCOMPRUEBA(vk.CreateFence(k.dev, &fci, NULL, &k.fence), "vkCreateFence");
    VkSubmitInfo si = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO, .commandBufferCount = 1, .pCommandBuffers = &cmd};
    VKCOMPRUEBA(vk.QueueSubmit(queue, 1, &si, k.fence), "vkQueueSubmit");
    // 20 s: con Vulkan por software y un traductor de por medio puede tardar; VK_TIMEOUT (2) queda como FALLO
    VKCOMPRUEBA(vk.WaitForFences(k.dev, 1, &k.fence, VK_TRUE, 20000000000ull), "vkWaitForFences");

    unsigned char *px = NULL;
    VKCOMPRUEBA(vk.MapMemory(k.dev, k.bmem, 0, VK_WHOLE_SIZE, 0, (void **)&px), "vkMapMemory");
    // dentro del triangulo (8,8) y fuera (56,56); R8G8B8A8: triangulo 255,128,64 y fondo 64,128,191
    const unsigned char *a = px + (8 * VK_LADO + 8) * 4, *b = px + (56 * VK_LADO + 56) * 4;
    int dentro[3] = {a[0], a[1], a[2]}, fuera[3] = {b[0], b[1], b[2]};
    vk.UnmapMemory(k.dev, k.bmem);
    int ok = abs(dentro[0] - 255) <= 2 && abs(dentro[1] - 128) <= 2 && abs(dentro[2] - 64) <= 2 && abs(fuera[0] - 64) <= 2 && abs(fuera[1] - 128) <= 2 &&
             abs(fuera[2] - 191) <= 2;
    snprintf(out, cap, "%s triangulo=%d,%d,%d fondo=%d,%d,%d dispositivo=[%.60s] dispositivos=%u api=%u.%u.%u", ok ? "OK" : "FALLO", dentro[0], dentro[1],
             dentro[2], fuera[0], fuera[1], fuera[2], props.deviceName, total, VK_VERSION_MAJOR(props.apiVersion), VK_VERSION_MINOR(props.apiVersion),
             VK_VERSION_PATCH(props.apiVersion));
    vk_destruir(&k);
#undef VKCOMPRUEBA
#undef VKMAL
}

JNIEXPORT jstring JNICALL FN(vulkan)(JNIEnv *env, jclass c) {
    (void)c;
    char out[320];
    vk_caso(out, sizeof out);
    return str(env, out);
}

// ------------------------------------------------------------------------------------------------ bench: escena GLES
// l) gles_escena. Patron de juego Unity 2D/3D ligero: por cuadro, matematica en C por sprite (T*R*S y vista-proyeccion = 2 mat4
// por sprite, 4 vertices transformados) y 8 llamadas de dibujo con matrices de vertices en memoria del cliente. Pbuffer fuera
// de pantalla de 256x256; sum = FNV de 16 pixeles leidos al final. ms por cuadro en una linea BENCHINFO.
#define GL_FRAMES 300
#define GL_SPRITES 2000
#define GL_BATCHES 8
typedef struct { float x, y; uint8_t c[4]; } GVert;
static GVert gverts[GL_SPRITES * 6];
static uint64_t b_gles_escena(void) {
    EGLDisplay d = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (d == EGL_NO_DISPLAY || !eglInitialize(d, NULL, NULL)) { bench_fail("gles: eglInitialize"); return 0; }
    EGLint cfg_attr[] = {EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE};
    EGLConfig cfg;
    EGLint n = 0;
    if (!eglChooseConfig(d, cfg_attr, &cfg, 1, &n) || n < 1) { bench_fail("gles: eglChooseConfig"); return 0; }
    EGLint pb_attr[] = {EGL_WIDTH, 256, EGL_HEIGHT, 256, EGL_NONE};
    EGLSurface surf = eglCreatePbufferSurface(d, cfg, pb_attr);
    EGLint ctx_attr[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    EGLContext ctx = eglCreateContext(d, cfg, EGL_NO_CONTEXT, ctx_attr);
    if (surf == EGL_NO_SURFACE || ctx == EGL_NO_CONTEXT || !eglMakeCurrent(d, surf, surf, ctx)) { bench_fail("gles: contexto"); return 0; }
    const char *vs = "attribute vec2 p; attribute vec4 c; varying lowp vec4 v; void main(){ gl_Position = vec4(p, 0.0, 1.0); v = c; }";
    const char *fs = "varying lowp vec4 v; void main(){ gl_FragColor = v; }";
    GLuint vsh = shader(GL_VERTEX_SHADER, vs), fsh = shader(GL_FRAGMENT_SHADER, fs), prog = glCreateProgram();
    glAttachShader(prog, vsh);
    glAttachShader(prog, fsh);
    glBindAttribLocation(prog, 0, "p");
    glBindAttribLocation(prog, 1, "c");
    glLinkProgram(prog);
    GLint linked = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &linked);
    if (!vsh || !fsh || !linked) { bench_fail("gles: sombreadores"); return 0; }
    glUseProgram(prog);
    glViewport(0, 0, 256, 256);
    glEnableVertexAttribArray(0);
    glEnableVertexAttribArray(1);
    // Vertices en un VBO reescrito cada cuadro (con matrices en memoria del cliente ANGLE acumulaba cientos de buferes
    // sin swap y el pbuffer agotaba la memoria de dispositivo: GL_OUT_OF_MEMORY, 80 s).
    GLuint vbo = 0;
    glGenBuffers(1, &vbo);
    glBindBuffer(GL_ARRAY_BUFFER, vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof gverts, NULL, GL_STREAM_DRAW);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, sizeof(GVert), (const void *)offsetof(GVert, x));
    glVertexAttribPointer(1, 4, GL_UNSIGNED_BYTE, GL_TRUE, sizeof(GVert), (const void *)offsetof(GVert, c));
    uint64_t s = BENCH_SEED ^ 0x61e5;
    static float spx[GL_SPRITES], spy[GL_SPRITES], svx[GL_SPRITES], svy[GL_SPRITES], sang[GL_SPRITES], ssz[GL_SPRITES];
    static uint8_t scol[GL_SPRITES][4];
    for (int i = 0; i < GL_SPRITES; i++) {
        float *f[4] = {&spx[i], &spy[i], &svx[i], &svy[i]};
        for (int k = 0; k < 4; k++) *f[k] = (float)(int32_t)(bench_xs(&s) >> 32) * (1.0f / 2147483648.0f);
        svx[i] *= 0.01f; svy[i] *= 0.01f;
        sang[i] = (float)(bench_xs(&s) & 1023) * 0.006f;
        ssz[i] = 0.01f + (float)(bench_xs(&s) & 255) * 0.0002f;
        for (int k = 0; k < 4; k++) scol[i][k] = k == 3 ? 255 : (uint8_t)bench_xs(&s);
    }
    static const float vp[16] = {0.9f, 0, 0, 0, 0, 0.9f, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
    static const float quad[4][4] = {{-1, -1, 0, 1}, {1, -1, 0, 1}, {1, 1, 0, 1}, {-1, 1, 0, 1}};
    static const int idx[6] = {0, 1, 2, 0, 2, 3};
    int64_t cpu_ns = 0;
    for (int fr = 0; fr < GL_FRAMES; fr++) {
        struct timespec t0, t1;
        clock_gettime(CLOCK_MONOTONIC, &t0);
        for (int i = 0; i < GL_SPRITES; i++) {
            spx[i] += svx[i]; spy[i] += svy[i]; sang[i] += 0.02f;
            if (spx[i] > 1.0f || spx[i] < -1.0f) svx[i] = -svx[i];
            if (spy[i] > 1.0f || spy[i] < -1.0f) svy[i] = -svy[i];
            float c = cosf(sang[i]) * ssz[i], sn = sinf(sang[i]) * ssz[i];
            float trs[16] = {c, sn, 0, 0, -sn, c, 0, 0, 0, 0, 1, 0, spx[i], spy[i], 0, 1}, mvp[16];
            bench_m4_mul(mvp, vp, trs);
            for (int v = 0; v < 6; v++) {
                const float *q = quad[idx[v]];
                GVert *g = &gverts[i * 6 + v];
                g->x = mvp[0] * q[0] + mvp[4] * q[1] + mvp[8] * q[2] + mvp[12] * q[3];
                g->y = mvp[1] * q[0] + mvp[5] * q[1] + mvp[9] * q[2] + mvp[13] * q[3];
                memcpy(g->c, scol[i], 4);
            }
        }
        clock_gettime(CLOCK_MONOTONIC, &t1);
        cpu_ns += (int64_t)(t1.tv_sec - t0.tv_sec) * 1000000000LL + (t1.tv_nsec - t0.tv_nsec);
        glBufferData(GL_ARRAY_BUFFER, sizeof gverts, gverts, GL_STREAM_DRAW);
        glClearColor(0, 0, 0, 1);
        glClear(GL_COLOR_BUFFER_BIT);
        for (int b = 0; b < GL_BATCHES; b++) glDrawArrays(GL_TRIANGLES, b * (GL_SPRITES / GL_BATCHES) * 6, (GL_SPRITES / GL_BATCHES) * 6);
        eglSwapBuffers(d, surf);  // en un pbuffer solo cierra el cuadro (libera recursos del cuadro anterior)
        if (fr % 50 == 49) glFinish();
    }
    uint64_t h = BENCH_FNV0;
    for (int k = 0; k < 16; k++) {
        unsigned char px[4] = {0};
        glReadPixels(8 + (k % 4) * 80, 8 + (k / 4) * 80, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
        h = bench_fnv(h, px, 4);
    }
    GLenum ge = glGetError();
    if (ge != GL_NO_ERROR) { bench_fail("gles: glGetError"); bench_info("gles glGetError=0x%x", (unsigned)ge); }
    glDeleteBuffers(1, &vbo);
    bench_info("gles_escena cuadros=%d cpu_mat4_ms=%lld", GL_FRAMES, (long long)(cpu_ns / 1000000));
    eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(d, ctx);
    eglDestroySurface(d, surf);
    return h;
}

// ------------------------------------------------------------------------------------------------ bench: JNI
// m) gles_pantalla. Superficie EGL de VENTANA (ANativeWindow del SurfaceView), la ruta real de un juego: swap con
// SurfaceFlinger. Cuatro fases de 30 s: (escena real | triangulo) x (eglSwapInterval 1 | 0). Cada 5 s: una linea
// "PANTALLA intervalo=<i> cuadros=<n> fps=<x> ms_cpu_por_cuadro=<y> escena=<e> ms_swap=<z>" y marcas "PANTALLA fase ...".
static double now_ms(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1000.0 + t.tv_nsec / 1e6; }
JNIEXPORT jint JNICALL FN(pantalla)(JNIEnv *env, jclass c, jobject surface, jint fase_s, jint ini) {
    ANativeWindow *win = ANativeWindow_fromSurface(env, surface);
    if (!win) { __android_log_print(ANDROID_LOG_ERROR, "banco", "PANTALLA fallo ANativeWindow"); return 1; }
    EGLDisplay d = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (d == EGL_NO_DISPLAY || !eglInitialize(d, NULL, NULL)) { __android_log_print(ANDROID_LOG_ERROR, "banco", "PANTALLA fallo eglInitialize"); return 2; }
    EGLint cfg_attr[] = {EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE};
    EGLConfig cfg;
    EGLint n = 0;
    if (!eglChooseConfig(d, cfg_attr, &cfg, 1, &n) || n < 1) { __android_log_print(ANDROID_LOG_ERROR, "banco", "PANTALLA fallo eglChooseConfig"); return 3; }
    EGLSurface surf = eglCreateWindowSurface(d, cfg, win, NULL);
    EGLint ctx_attr[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    EGLContext ctx = eglCreateContext(d, cfg, EGL_NO_CONTEXT, ctx_attr);
    if (surf == EGL_NO_SURFACE || ctx == EGL_NO_CONTEXT || !eglMakeCurrent(d, surf, surf, ctx)) { __android_log_print(ANDROID_LOG_ERROR, "banco", "PANTALLA fallo contexto 0x%x", (unsigned)eglGetError()); return 4; }
    EGLint W = 0, H = 0;
    eglQuerySurface(d, surf, EGL_WIDTH, &W);
    eglQuerySurface(d, surf, EGL_HEIGHT, &H);
    __android_log_print(ANDROID_LOG_INFO, "banco", "PANTALLA inicio superficie=%dx%d", W, H);
    const char *vs = "attribute vec2 p; attribute vec4 c; varying lowp vec4 v; void main(){ gl_Position = vec4(p, 0.0, 1.0); v = c; }";
    const char *fs = "varying lowp vec4 v; void main(){ gl_FragColor = v; }";
    GLuint vsh = shader(GL_VERTEX_SHADER, vs), fsh = shader(GL_FRAGMENT_SHADER, fs), prog = glCreateProgram();
    glAttachShader(prog, vsh);
    glAttachShader(prog, fsh);
    glBindAttribLocation(prog, 0, "p");
    glBindAttribLocation(prog, 1, "c");
    glLinkProgram(prog);
    glUseProgram(prog);
    glViewport(0, 0, W, H);
    glEnableVertexAttribArray(0);
    glEnableVertexAttribArray(1);
    GLuint vbo = 0;
    glGenBuffers(1, &vbo);
    glBindBuffer(GL_ARRAY_BUFFER, vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof gverts, NULL, GL_STREAM_DRAW);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, sizeof(GVert), (const void *)offsetof(GVert, x));
    glVertexAttribPointer(1, 4, GL_UNSIGNED_BYTE, GL_TRUE, sizeof(GVert), (const void *)offsetof(GVert, c));
    uint64_t s = BENCH_SEED ^ 0x61e5;
    static float spx[GL_SPRITES], spy[GL_SPRITES], svx[GL_SPRITES], svy[GL_SPRITES], sang[GL_SPRITES], ssz[GL_SPRITES];
    static uint8_t scol[GL_SPRITES][4];
    for (int i = 0; i < GL_SPRITES; i++) {
        float *f[4] = {&spx[i], &spy[i], &svx[i], &svy[i]};
        for (int k = 0; k < 4; k++) *f[k] = (float)(int32_t)(bench_xs(&s) >> 32) * (1.0f / 2147483648.0f);
        svx[i] *= 0.01f; svy[i] *= 0.01f;
        sang[i] = (float)(bench_xs(&s) & 1023) * 0.006f;
        ssz[i] = 0.01f + (float)(bench_xs(&s) & 255) * 0.0002f;
        for (int k = 0; k < 4; k++) scol[i][k] = k == 3 ? 255 : (uint8_t)bench_xs(&s);
    }
    static const float vp[16] = {0.9f, 0, 0, 0, 0, 0.9f, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
    static const float quad[4][4] = {{-1, -1, 0, 1}, {1, -1, 0, 1}, {1, 1, 0, 1}, {-1, 1, 0, 1}};
    static const int idx[6] = {0, 1, 2, 0, 2, 3};
    static const GVert tri[3] = {{-0.5f, -0.5f, {255, 0, 0, 255}}, {0.5f, -0.5f, {0, 255, 0, 255}}, {0.0f, 0.5f, {0, 0, 255, 255}}};
    // fases extra: escena real, intervalo 1 y ANativeWindow_setFrameRate(30 / 60) (la peticion que hace un motor de juego)
    typedef int32_t (*setfr_t)(ANativeWindow *, float, int8_t);
    setfr_t setfr = (setfr_t)dlsym(RTLD_DEFAULT, "ANativeWindow_setFrameRate");
    static const int seq_all[6] = {0, 1, 2, 3, 4, 5}, seq_fr[2] = {5, 4};
    const int *seq = ini >= 4 ? seq_fr : seq_all;
    for (int ki = 0; ki < (ini >= 4 ? 2 : 6); ki++) {
        int fi = seq[ki];
        {
            int escena = fi < 2 ? 0 : fi < 4 ? 1 : 0;
            int ph = fi < 4 ? fi % 2 : 0;
            float rate = fi == 4 ? 30.0f : fi == 5 ? 60.0f : 0.0f;
            int interval = ph == 0 ? 1 : 0;
            const char *en = escena == 0 ? "real" : "triangulo";
            eglSwapInterval(d, interval);
            int fase_ms = fi < 4 ? fase_s * 1000 : 15000;
            if (rate > 0.0f) {
                int32_t r = setfr ? setfr(win, rate, 0) : -99;
                __android_log_print(ANDROID_LOG_INFO, "banco", "PANTALLA setFrameRate(%.0f)=%d", rate, r);
            }
            __android_log_print(ANDROID_LOG_INFO, "banco", "PANTALLA fase escena=%s intervalo=%d setfr=%.0f", en, interval, rate);
            double t_ini = now_ms(), t_win = t_ini, cpu_win = 0, swap_win = 0;
            long frames_win = 0, frames_all = 0;
            while (now_ms() - t_ini < fase_ms) {
                if (escena == 0) {
                    double a = now_ms();
                    for (int i = 0; i < GL_SPRITES; i++) {
                        spx[i] += svx[i]; spy[i] += svy[i]; sang[i] += 0.02f;
                        if (spx[i] > 1.0f || spx[i] < -1.0f) svx[i] = -svx[i];
                        if (spy[i] > 1.0f || spy[i] < -1.0f) svy[i] = -svy[i];
                        float cc = cosf(sang[i]) * ssz[i], sn = sinf(sang[i]) * ssz[i];
                        float trs[16] = {cc, sn, 0, 0, -sn, cc, 0, 0, 0, 0, 1, 0, spx[i], spy[i], 0, 1}, mvp[16];
                        bench_m4_mul(mvp, vp, trs);
                        for (int v = 0; v < 6; v++) {
                            const float *q = quad[idx[v]];
                            GVert *g = &gverts[i * 6 + v];
                            g->x = mvp[0] * q[0] + mvp[4] * q[1] + mvp[8] * q[2] + mvp[12] * q[3];
                            g->y = mvp[1] * q[0] + mvp[5] * q[1] + mvp[9] * q[2] + mvp[13] * q[3];
                            memcpy(g->c, scol[i], 4);
                        }
                    }
                    cpu_win += now_ms() - a;
                    glBufferData(GL_ARRAY_BUFFER, sizeof gverts, gverts, GL_STREAM_DRAW);
                    glClearColor(0, 0, 0, 1);
                    glClear(GL_COLOR_BUFFER_BIT);
                    for (int b = 0; b < GL_BATCHES; b++) glDrawArrays(GL_TRIANGLES, b * (GL_SPRITES / GL_BATCHES) * 6, (GL_SPRITES / GL_BATCHES) * 6);
                } else {
                    glBufferData(GL_ARRAY_BUFFER, sizeof tri, tri, GL_STREAM_DRAW);
                    float sh = (float)(frames_all % 60) / 60.0f;
                    glClearColor(sh, 0, 0.2f, 1);
                    glClear(GL_COLOR_BUFFER_BIT);
                    glDrawArrays(GL_TRIANGLES, 0, 3);
                }
                double b0 = now_ms();
                eglSwapBuffers(d, surf);
                swap_win += now_ms() - b0;
                frames_win++; frames_all++;
                double t = now_ms();
                if (t - t_win >= 5000.0) {
                    __android_log_print(ANDROID_LOG_INFO, "banco", "PANTALLA intervalo=%d cuadros=%ld fps=%.1f ms_cpu_por_cuadro=%.3f escena=%s ms_swap=%.3f setfr=%.0f",
                                        interval, frames_win, frames_win * 1000.0 / (t - t_win), cpu_win / (double)frames_win, en, swap_win / (double)frames_win, rate);
                    t_win = t; frames_win = 0; cpu_win = 0; swap_win = 0;
                }
            }
        }
    }
    __android_log_print(ANDROID_LOG_INFO, "banco", "PANTALLA fin");
    eglMakeCurrent(d, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(d, ctx);
    eglDestroySurface(d, surf);
    ANativeWindow_release(win);
    return 0;
}

static JNIEnv *g_env;
static jclass g_cls;
static jmethodID g_progress;
static void bench_out(const char *line) {
    __android_log_print(ANDROID_LOG_INFO, "banco", "%s", line);
    if (g_env && g_progress && strncmp(line, "BENCH ", 6) == 0) {  // progreso en pantalla: entre cargas, nunca dentro
        jstring js = (*g_env)->NewStringUTF(g_env, line + 6);
        (*g_env)->CallStaticVoidMethod(g_env, g_cls, g_progress, js);
        (*g_env)->DeleteLocalRef(g_env, js);
    }
}
static void bench_begin(const char *name) {
    if (!g_env || !g_progress) return;
    char buf[100];
    snprintf(buf, sizeof buf, "ejecutando: %s ...", name);
    jstring js = (*g_env)->NewStringUTF(g_env, buf);
    (*g_env)->CallStaticVoidMethod(g_env, g_cls, g_progress, js);
    (*g_env)->DeleteLocalRef(g_env, js);
}

// Se llama desde el hilo "bench" de Java (el hilo ya es nativo): ejecuta la bateria y la escena GLES. Devuelve ok.
JNIEXPORT jint JNICALL FN(bench)(JNIEnv *env, jclass c, jint rep, jstring solo) {
    g_env = env;
    g_cls = c;
    g_progress = (*env)->GetStaticMethodID(env, c, "progress", "(Ljava/lang/String;)V");
    (*env)->ExceptionClear(env);
    bench_set_begin(bench_begin);
    static const bench_def extra[] = {{"gles_escena", b_gles_escena}};
    const char *only = solo ? (*env)->GetStringUTFChars(env, solo, NULL) : NULL;
    int ok = bench_run(rep, bench_out, extra, 1, only);
    if (only) (*env)->ReleaseStringUTFChars(env, solo, only);
    return ok;
}

// ------------------------------------------------------------------------------------------------ bajo nivel
// Casos que un traductor de ARM suele romper y que apps reales usan: recuperarse de un fallo de memoria (Mono, Xamarin,
// Unity, ART con sus comprobaciones implicitas), generar codigo propio (JIT de JavaScript o Lua), mascaras de senales
// por hilo y metodos nativos registrados con firmas mezcladas de enteros y coma flotante.
#include <setjmp.h>
#include <signal.h>
#include <sys/mman.h>
#include <ucontext.h>
#include <unistd.h>

static sigjmp_buf salto;
static volatile sig_atomic_t fallos_vistos;
static volatile uintptr_t direccion_vista;
static volatile int recuperado_por_pc;

static void al_fallar_salto(int sig, siginfo_t *si, void *uc) {
    (void)sig; (void)uc;
    fallos_vistos++;
    direccion_vista = (uintptr_t)si->si_addr;
    siglongjmp(salto, 1);
}

// la funcion a la que el manejador manda el pc (como hacen los runtimes que convierten un SEGV en una excepcion)
static void __attribute__((noinline)) recuperacion(void) {
    recuperado_por_pc = 1;
    siglongjmp(salto, 2);
}

static void al_fallar_pc(int sig, siginfo_t *si, void *ucv) {
    (void)sig; (void)si;
    ucontext_t *uc = ucv;
    fallos_vistos++;
#if defined(__aarch64__)
    uc->uc_mcontext.pc = (uintptr_t)recuperacion;
#elif defined(__x86_64__)
    uc->uc_mcontext.gregs[REG_RIP] = (greg_t)(uintptr_t)recuperacion;
#endif
}

static int provocar_fallo(void) {
    volatile int *p = (volatile int *)(uintptr_t)0x10;
    return *p;
}

JNIEXPORT jstring JNICALL FN(fallos)(JNIEnv *env, jclass c) {
    (void)c;
    char out[200];
    struct sigaction sa, viejo;
    memset(&sa, 0, sizeof sa);
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    // 1) el manejador sale con siglongjmp
    fallos_vistos = 0;
    sa.sa_sigaction = al_fallar_salto;
    sigaction(SIGSEGV, &sa, &viejo);
    int salto_ok = 0;
    if (sigsetjmp(salto, 1) == 0) {
        provocar_fallo();
    } else {
        salto_ok = fallos_vistos == 1 && direccion_vista == 0x10;
    }
    // 2) el manejador cambia el pc del contexto y vuelve (lo que hace Mono)
    fallos_vistos = 0;
    recuperado_por_pc = 0;
    sa.sa_sigaction = al_fallar_pc;
    sa.sa_flags = SA_SIGINFO | SA_NODEFER;
    sigaction(SIGSEGV, &sa, NULL);
    int pc_ok = 0;
    if (sigsetjmp(salto, 1) == 0) {
        provocar_fallo();
    } else {
        pc_ok = fallos_vistos == 1 && recuperado_por_pc;
    }
    sigaction(SIGSEGV, &viejo, NULL);
    snprintf(out, sizeof out, "%s siglongjmp=%d contexto_pc=%d", salto_ok && pc_ok ? "OK" : "FALLO", salto_ok, pc_ok);
    return str(env, out);
}

// Escribe una funcion que devuelve `v` en `m` (codigo de la arquitectura de esta biblioteca).
static size_t emitir_devuelve(unsigned char *m, uint32_t v) {
#if defined(__aarch64__)
    uint32_t ins[3] = {0x52800000u | ((v & 0xffff) << 5), 0x72a00000u | ((v >> 16) << 5), 0xd65f03c0u}; // movz w0; movk w0, lsl 16; ret
    memcpy(m, ins, sizeof ins);
    return sizeof ins;
#elif defined(__x86_64__)
    m[0] = 0xb8; memcpy(m + 1, &v, 4); m[5] = 0xc3; // mov eax, v; ret
    return 6;
#else
    (void)m; (void)v; return 0;
#endif
}

JNIEXPORT jstring JNICALL FN(codigo)(JNIEnv *env, jclass c) {
    (void)c;
    char out[200];
    size_t pg = (size_t)sysconf(_SC_PAGESIZE);
    unsigned char *m = mmap(NULL, pg, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) return str(env, "FALLO mmap");
    typedef uint32_t (*fn)(void);
    uint32_t r[3] = {0, 0, 0};
    uint32_t esperado[3] = {0x12345678u, 0x0badcafeu, 7u};
    for (int i = 0; i < 3; i++) {
        // escribir, hacerlo ejecutable, invalidar la cache de instrucciones y llamar: cada vez con otro valor en la
        // misma direccion (un traductor que no invalide devolveria el valor anterior)
        mprotect(m, pg, PROT_READ | PROT_WRITE);
        size_t n = emitir_devuelve(m, esperado[i]);
        mprotect(m, pg, PROT_READ | PROT_EXEC);
        __builtin___clear_cache((char *)m, (char *)m + n);
        r[i] = ((fn)(void *)m)();
    }
    munmap(m, pg);
    int ok = r[0] == esperado[0] && r[1] == esperado[1] && r[2] == esperado[2];
    snprintf(out, sizeof out, "%s valores=%x,%x,%x", ok ? "OK" : "FALLO", r[0], r[1], r[2]);
    return str(env, out);
}

static volatile sig_atomic_t usr2_vistas;
static void al_usr2(int s) { (void)s; usr2_vistas++; }

JNIEXPORT jstring JNICALL FN(mascara)(JNIEnv *env, jclass c) {
    (void)c;
    char out[200];
    struct sigaction sa, viejo;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = al_usr2;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR2, &sa, &viejo);
    sigset_t b, antes, pendientes;
    sigemptyset(&b);
    sigaddset(&b, SIGUSR2);
    usr2_vistas = 0;
    pthread_sigmask(SIG_BLOCK, &b, &antes);
    sigset_t ahora;
    pthread_sigmask(SIG_BLOCK, NULL, &ahora);
    int bloqueada = sigismember(&ahora, SIGUSR2) == 1;
    // bloqueada: queda pendiente y el manejador no corre
    pthread_kill(pthread_self(), SIGUSR2);
    sigpending(&pendientes);
    int pendiente = sigismember(&pendientes, SIGUSR2) == 1 && usr2_vistas == 0;
    // al desbloquear se entrega
    pthread_sigmask(SIG_SETMASK, &antes, NULL);
    for (int i = 0; i < 100 && usr2_vistas == 0; i++) usleep(1000);
    int entregada = usr2_vistas == 1;
    sigaction(SIGUSR2, &viejo, NULL);
    int ok = bloqueada && pendiente && entregada;
    snprintf(out, sizeof out, "%s bloqueada=%d pendiente=%d entregada=%d", ok ? "OK" : "FALLO", bloqueada, pendiente, entregada);
    return str(env, out);
}

// Registrado con RegisterNatives (no por nombre): enteros y coma flotante intercalados, mas de los que caben en registros.
static jstring mezcla(JNIEnv *env, jclass c, jint a, jlong b, jfloat f, jdouble d, jint e, jfloat g, jdouble h, jlong i,
                      jfloat j, jdouble k, jint l, jfloat m2, jdouble n, jlong o, jfloat p, jdouble q, jint r, jdouble s) {
    (void)c;
    char out[300];
    double suma = a + (double)b + f + d + e + g + h + (double)i + j + k + l + m2 + n + (double)o + p + q + r + s;
    int ok = a == 1 && b == 2000000000000LL && f == 3.5f && d == 4.25 && e == -5 && g == 6.5f && h == 7.125 && i == -8 &&
             j == 9.5f && k == 10.75 && l == 11 && m2 == 12.5f && n == 13.25 && o == 14 && p == 15.5f && q == 16.125 && r == 17 &&
             s == 18.5;
    snprintf(out, sizeof out, "%s suma=%.3f", ok ? "OK" : "FALLO", suma);
    return str(env, out);
}

JNIEXPORT jint JNI_OnLoad(JavaVM *vm, void *reservado) {
    (void)reservado;
    JNIEnv *env;
    if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
    jclass k = (*env)->FindClass(env, "rs/weft/banco/Main");
    if (k == NULL) return JNI_ERR;
    JNINativeMethod met[] = {{"mezcla", "(IJFDIFDJFDIFDJFDID)Ljava/lang/String;", (void *)mezcla}};
    if ((*env)->RegisterNatives(env, k, met, 1) != 0) {
        // sin el registro solo falla la prueba jni_registrado (UnsatisfiedLinkError); el resto del banco sigue
        (*env)->ExceptionClear(env);
        __android_log_print(ANDROID_LOG_WARN, "banco", "RegisterNatives fallo");
    }
    return JNI_VERSION_1_6;
}
