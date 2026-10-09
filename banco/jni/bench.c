// Cargas del banco de rendimiento. Cada carga devuelve un checksum FNV-1a de 64 bits de sus resultados, que debe ser
// identico en toda ejecucion y toda version del traductor (prueba de equivalencia). Los tiempos los mide bench_run.
// Se compila con -O2; en arm64 con -mno-outline-atomics para que __atomic_* salga como LDAXR/STLXR (LL/SC).
#define _GNU_SOURCE
#include "bench.h"

#include <math.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#if defined(__aarch64__)
#include <sys/auxv.h>
#endif

// ------------------------------------------------------------------------------------------------ utilidades
static bench_emit g_emit;
static void (*g_begin)(const char *);
void bench_set_begin(void (*f)(const char *)) { g_begin = f; }
static int g_ok = 1;

void bench_fail(const char *what) {
    g_ok = 0;
    bench_info("FALLO %s", what);
}

void bench_info(const char *fmt, ...) {
    char buf[300];
    int n = snprintf(buf, sizeof buf, "BENCHINFO ");
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf + n, sizeof buf - n, fmt, ap);
    va_end(ap);
    if (g_emit) g_emit(buf);
}

uint64_t bench_xs(uint64_t *s) {
    uint64_t x = *s;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    return *s = x;
}

uint64_t bench_fnv(uint64_t h, const void *p, uint64_t n) {
    const uint8_t *b = p;
    uint64_t i = 0;
    for (; i + 8 <= n; i += 8) {
        uint64_t w;
        memcpy(&w, b + i, 8);
        h = (h ^ w) * 0x100000001b3ULL;
    }
    for (; i < n; i++) h = (h ^ b[i]) * 0x100000001b3ULL;
    return h;
}
static uint64_t fnv64(uint64_t h, uint64_t v) { return bench_fnv(h, &v, 8); }
#define xs bench_xs
static float rf(uint64_t *s) { return (float)(int32_t)(xs(s) >> 32) * (1.0f / 2147483648.0f); }  // [-1, 1)

static void run_threads(int n, void *(*f)(void *)) {
    pthread_t th[16];
    for (int i = 0; i < n; i++) pthread_create(&th[i], NULL, f, (void *)(intptr_t)i);
    for (int i = 0; i < n; i++) pthread_join(th[i], NULL);
}

// ------------------------------------------------------------------------------------------------ a) mat4
// Unity: transformaciones de objetos (Transform, matrices de mundo, skinning). Producto 4x4 columna-mayor y matriz*vector:
// el compilador lo vectoriza en FMUL/FMLA (vector por elemento) y FADD vectorial; el recorte usa FMINNM/FMAXNM.
#define MAT_N 2048
#define MAT_PASSES 300
typedef struct { float m[16]; } M4;
void bench_m4_mul(float *o, const float *a, const float *b) {
    for (int c = 0; c < 4; c++)
        for (int r = 0; r < 4; r++) o[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
}
static M4 mA[MAT_N], mB[MAT_N], mC[MAT_N];
static float mV[MAT_N][4];
static uint64_t b_mat4(void) {
    uint64_t s = BENCH_SEED;
    for (int i = 0; i < MAT_N; i++) {
        for (int k = 0; k < 16; k++) { mA[i].m[k] = rf(&s) * 0.5f; mB[i].m[k] = rf(&s) * 0.5f; }
        for (int k = 0; k < 4; k++) mV[i][k] = rf(&s);
    }
    for (int p = 0; p < MAT_PASSES; p++) {
        for (int i = 0; i < MAT_N; i++) bench_m4_mul(mC[i].m, mA[i].m, mB[(i + p) & (MAT_N - 1)].m);
        for (int i = 0; i < MAT_N; i++) {
            for (int k = 0; k < 16; k++) {
                float v = mC[i].m[k] * 1.5f + mB[i].m[k] * 0.5f;
                mA[i].m[k] = fminf(fmaxf(v, -1.0f), 1.0f);
            }
            float *m = mA[i].m, *v = mV[i], r[4];
            for (int k = 0; k < 4; k++) r[k] = m[k] * v[0] + m[4 + k] * v[1] + m[8 + k] * v[2] + m[12 + k] * v[3];
            for (int k = 0; k < 4; k++) v[k] = fminf(fmaxf(r[k] + 0.25f * v[k], -2.0f), 2.0f);
        }
    }
    uint64_t h = BENCH_FNV0;
    h = bench_fnv(h, mA, sizeof mA);
    return bench_fnv(h, mV, sizeof mV);
}

// ------------------------------------------------------------------------------------------------ b) fisica
// Unity: Physics/particulas simples (integracion de Euler, gravedad, rebote contra suelo y paredes). Bucle vectorizable con
// comparaciones y seleccion (FCMGT/BSL, FCMP+FCSEL) y una reduccion escalar con FSQRT/FADD en cadena (FP escalar).
#define PH_N 10000
#define PH_STEPS 400
static float px[PH_N], py[PH_N], pz[PH_N], pvx[PH_N], pvy[PH_N], pvz[PH_N];
static uint64_t b_fisica(void) {
    uint64_t s = BENCH_SEED ^ 0x1234;
    for (int i = 0; i < PH_N; i++) {
        px[i] = rf(&s) * 50.0f; py[i] = rf(&s) * 50.0f + 50.0f; pz[i] = rf(&s) * 50.0f;
        pvx[i] = rf(&s) * 5.0f; pvy[i] = rf(&s) * 5.0f; pvz[i] = rf(&s) * 5.0f;
    }
    const float dt = 1.0f / 60.0f;
    float energy = 0.0f;
    for (int st = 0; st < PH_STEPS; st++) {
        for (int i = 0; i < PH_N; i++) {  // integracion (vectorizable: FMUL/FMLA/FADD vectoriales)
            pvy[i] -= 9.81f * dt;
            pvx[i] *= 0.9995f;
            pvz[i] *= 0.9995f;
            px[i] += pvx[i] * dt;
            py[i] += pvy[i] * dt;
            pz[i] += pvz[i] * dt;
        }
        for (int i = 0; i < PH_N; i++) {  // rebotes sin ramas (comparacion + seleccion)
            float y = py[i], vy = pvy[i];
            int neg = y < 0.0f;
            py[i] = neg ? -y : y;
            pvy[i] = neg ? -vy * 0.8f : vy;
            float x = px[i], vx = pvx[i];
            int hi = x > 50.0f, lo = x < -50.0f;
            px[i] = hi ? 100.0f - x : (lo ? -100.0f - x : x);
            pvx[i] = (hi | lo) ? -vx : vx;
            float z = pz[i], vz = pvz[i];
            hi = z > 50.0f; lo = z < -50.0f;
            pz[i] = hi ? 100.0f - z : (lo ? -100.0f - z : z);
            pvz[i] = (hi | lo) ? -vz : vz;
        }
        for (int i = 0; i < PH_N; i++) {  // rama escalar por particula (FCMP + salto/FCSEL)
            if (fabsf(pvx[i]) < 0.001f) pvx[i] = 0.5f;
            pvy[i] = pvy[i] > 80.0f ? 80.0f : pvy[i];
        }
        for (int i = 0; i < PH_N; i += 7) energy += sqrtf(pvx[i] * pvx[i] + pvy[i] * pvy[i] + pvz[i] * pvz[i]);  // reduccion escalar
    }
    uint64_t h = BENCH_FNV0;
    h = bench_fnv(h, px, sizeof px); h = bench_fnv(h, py, sizeof py); h = bench_fnv(h, pz, sizeof pz);
    h = bench_fnv(h, pvx, sizeof pvx); h = bench_fnv(h, pvy, sizeof pvy); h = bench_fnv(h, pvz, sizeof pvz);
    return bench_fnv(h, &energy, sizeof energy);
}

// ------------------------------------------------------------------------------------------------ c) particulas_mem
// Unity: buferes de particulas/vertices/instancias grandes, reescritos cada cuadro (stores masivos de 64 bytes: LDP/STP de Q
// y LD1/ST1 por la vectorizacion), mas lectura de todo el bufer y memset/memcpy de bloques.
#define PM_N 49152
#define PM_STEPS 200
typedef struct { float pos[4]; float vel[4]; float col[4]; uint32_t id, life, flags, pad; } Part;  // 64 bytes
static Part pbuf[2][PM_N];
static uint64_t b_particulas_mem(void) {
    uint64_t s = BENCH_SEED ^ 0x77;
    for (int i = 0; i < PM_N; i++) {
        Part *p = &pbuf[0][i];
        for (int k = 0; k < 4; k++) { p->pos[k] = rf(&s); p->vel[k] = rf(&s) * 0.1f; p->col[k] = (rf(&s) + 1.0f) * 0.5f; }
        p->id = i; p->life = 10 + (uint32_t)(xs(&s) % 50); p->flags = 0; p->pad = 0;
    }
    uint64_t acc = 0;
    for (int st = 0; st < PM_STEPS; st++) {
        Part *src = pbuf[st & 1], *dst = pbuf[(st & 1) ^ 1];
        for (int i = 0; i < PM_N; i++) {
            Part p = src[i];
            for (int k = 0; k < 4; k++) { p.pos[k] += p.vel[k] * 0.016f; p.col[k] *= 0.99f; }
            if (--p.life == 0) { p.life = 40; for (int k = 0; k < 4; k++) p.pos[k] = 0.0f; p.flags++; }
            dst[i] = p;
        }
        for (int i = 0; i < PM_N; i++) acc += dst[i].life + dst[i].flags;
        memcpy(src, dst, sizeof(Part) * 4096);  // bloque adicional (memcpy grande)
        memset(&src[PM_N - 1024], 0, sizeof(Part) * 512);
    }
    uint64_t h = fnv64(BENCH_FNV0, acc);
    return bench_fnv(h, pbuf[PM_STEPS & 1], sizeof(Part) * PM_N);
}

// ------------------------------------------------------------------------------------------------ pila sin bloqueos
// Pila de Treiber con indices de 32 bits y etiqueta (contador ABA) en los 32 bits altos de la cabeza de 64 bits: CAS debil
// en bucle (LDAXR/STLXR con -mno-outline-atomics). Entre pop y push el hilo hace un store propio al nodo (patron del
// cuelgue observado: LDXR de la cabeza, store a otra direccion, STXR).
typedef struct { uint32_t next; uint32_t pad; uint64_t val; char pad2[48]; } SNode;  // 64 bytes: un granulo por nodo
typedef struct { _Alignas(128) uint64_t head; } Stack;
static Stack S;
static SNode pool[64];
static uint32_t g_iters;
static void st_push(uint32_t idx) {
    uint64_t old = __atomic_load_n(&S.head, __ATOMIC_RELAXED), nw;
    do {
        __atomic_store_n(&pool[idx].next, (uint32_t)old, __ATOMIC_RELAXED);
        nw = (((old >> 32) + 1) << 32) | (idx + 1);
    } while (!__atomic_compare_exchange_n(&S.head, &old, nw, 1, __ATOMIC_RELEASE, __ATOMIC_RELAXED));
}
static int st_pop(void) {
    uint64_t old = __atomic_load_n(&S.head, __ATOMIC_ACQUIRE);
    for (;;) {
        uint32_t ix = (uint32_t)old;
        if (!ix) return -1;
        uint32_t nx = __atomic_load_n(&pool[ix - 1].next, __ATOMIC_RELAXED);
        uint64_t nw = (((old >> 32) + 1) << 32) | nx;
        if (__atomic_compare_exchange_n(&S.head, &old, nw, 1, __ATOMIC_ACQUIRE, __ATOMIC_ACQUIRE)) return (int)(ix - 1);
    }
}
static void st_init(int nodes) {
    memset(&S, 0, sizeof S);
    memset(pool, 0, sizeof pool);
    for (int i = 0; i < nodes; i++) st_push(i);
}
static void st_work(int t) {
    for (uint32_t it = 0; it < g_iters; it++) {
        int ix;
        while ((ix = st_pop()) < 0) sched_yield();
        pool[ix].val += (uint64_t)(t + 1) * ((it & 15) + 1);  // store propio entre pop y push
        st_push((uint32_t)ix);
    }
}
// Vacia la pila, comprueba el conteo y la suma exacta y devuelve el checksum.
static uint64_t st_check(int nthreads, int nodes, const char *name) {
    uint64_t exp = 0, per = 0;
    for (uint32_t it = 0; it < g_iters; it++) per += (it & 15) + 1;
    for (int t = 0; t < nthreads; t++) exp += (uint64_t)(t + 1) * per;
    uint64_t total = 0, count = 0;
    int ix;
    while ((ix = st_pop()) >= 0) { total += pool[ix].val; count++; }
    if (count != (uint64_t)nodes || total != exp) { bench_fail(name); bench_info("%s conteo=%llu/%d suma=%llu/%llu", name, (unsigned long long)count, nodes, (unsigned long long)total, (unsigned long long)exp); }
    return fnv64(fnv64(BENCH_FNV0, count), total);
}
static void *st_thread(void *a) { st_work((int)(intptr_t)a); return NULL; }

// d) pila_lockfree: 8 hilos x 200 000 pop/store/push sobre una unica pila.
static uint64_t b_pila_lockfree(void) {
    g_iters = 200000;
    st_init(32);
    run_threads(8, st_thread);
    return st_check(8, 32, "pila_lockfree");
}

// ------------------------------------------------------------------------------------------------ e) atomicos
// Unity/il2cpp: contadores y banderas atomicos (Interlocked, referencias, colas): __atomic_fetch_add/xor sobre un contador
// compartido, sobre lineas de cache propias y sobre un arreglo empaquetado (falso compartido). LL/SC en armv8-a; la
// variante atomicos_lse usa target("lse") (LDADD/LDEOR) y solo corre si el procesador lo informa.
#define AT_T 8
#define AT_ITERS 400000
static struct {
    _Alignas(128) uint64_t shared;
    _Alignas(128) uint64_t line[AT_T][16];
    _Alignas(128) uint64_t packed[AT_T];
    _Alignas(128) uint64_t xr;
} at;
#define ATOM_BODY                                                         \
    int t = (int)(intptr_t)a;                                             \
    for (uint64_t i = 0; i < AT_ITERS; i++) {                             \
        __atomic_fetch_add(&at.shared, 1, __ATOMIC_RELAXED);              \
        __atomic_fetch_add(&at.line[t][0], i, __ATOMIC_SEQ_CST);          \
        __atomic_fetch_add(&at.packed[t], 1, __ATOMIC_ACQ_REL);           \
        __atomic_fetch_xor(&at.xr, i * (uint64_t)(t + 1), __ATOMIC_RELAXED); \
    }                                                                     \
    return NULL;
static void *at_thread(void *a) { ATOM_BODY }
#if defined(__aarch64__)
__attribute__((target("lse"))) static void *at_thread_lse(void *a) { ATOM_BODY }
#endif
static uint64_t at_run(void *(*f)(void *), const char *name) {
    memset(&at, 0, sizeof at);
    run_threads(AT_T, f);
    uint64_t xr = 0, ok = at.shared == (uint64_t)AT_T * AT_ITERS;
    for (int t = 0; t < AT_T; t++) {
        for (uint64_t i = 0; i < AT_ITERS; i++) xr ^= i * (uint64_t)(t + 1);
        ok &= at.line[t][0] == (uint64_t)AT_ITERS * (AT_ITERS - 1) / 2 && at.packed[t] == AT_ITERS;
    }
    ok &= at.xr == xr;
    if (!ok) bench_fail(name);
    uint64_t h = fnv64(BENCH_FNV0, at.shared);
    h = bench_fnv(h, at.line, sizeof at.line);
    h = bench_fnv(h, at.packed, sizeof at.packed);
    return fnv64(h, at.xr);
}
static uint64_t b_atomicos(void) { return at_run(at_thread, "atomicos"); }
#if defined(__aarch64__)
static uint64_t b_atomicos_lse(void) {
    if (!(getauxval(AT_HWCAP) & (1UL << 8))) {  // HWCAP_ATOMICS
        bench_info("atomicos_lse omitido: HWCAP_ATOMICS ausente");
        return 0;
    }
    return at_run(at_thread_lse, "atomicos_lse");
}
#endif

// ------------------------------------------------------------------------------------------------ f) trabajos
// Unity Job System / il2cpp: 8 hilos trabajadores con cola, espera por condvar (futex) y sched_yield antes de dormir;
// trabajos muy pequenos (100 000). Mucho pthread_mutex/cond, atomicos de contabilidad y cambios de hilo.
#define JB_THREADS 8
#define JB_JOBS 400000
#define JB_QCAP 1024
static struct {
    pthread_mutex_t mu;
    pthread_cond_t work, space;
    uint32_t q[JB_QCAP];
    uint32_t head, tail, count;
    int quit;
    _Alignas(128) _Atomic uint64_t result;
    _Alignas(128) _Atomic uint32_t done;
} jb;
static void *jb_worker(void *a) {
    (void)a;
    for (;;) {
        pthread_mutex_lock(&jb.mu);
        if (jb.count == 0 && !jb.quit) {
            pthread_mutex_unlock(&jb.mu);
            for (int k = 0; k < 8; k++) {  // espera activa corta con cesion del hilo antes de dormir
                sched_yield();
                if (atomic_load_explicit((_Atomic uint32_t *)&jb.count, memory_order_relaxed)) break;
            }
            pthread_mutex_lock(&jb.mu);
            while (jb.count == 0 && !jb.quit) pthread_cond_wait(&jb.work, &jb.mu);
        }
        if (jb.count == 0) { pthread_mutex_unlock(&jb.mu); return NULL; }
        uint32_t id = jb.q[jb.head];
        jb.head = (jb.head + 1) % JB_QCAP;
        jb.count--;
        pthread_cond_signal(&jb.space);
        pthread_mutex_unlock(&jb.mu);
        uint64_t h = id + 1, r = 0;
        for (int k = 0; k < 32; k++) { bench_xs(&h); r += h & 0xffff; }  // trabajo pequeno
        atomic_fetch_add(&jb.result, r);
        atomic_fetch_add(&jb.done, 1);
    }
}
static uint64_t b_trabajos(void) {
    memset(&jb, 0, sizeof jb);
    pthread_mutex_init(&jb.mu, NULL);
    pthread_cond_init(&jb.work, NULL);
    pthread_cond_init(&jb.space, NULL);
    pthread_t th[JB_THREADS];
    for (int i = 0; i < JB_THREADS; i++) pthread_create(&th[i], NULL, jb_worker, NULL);
    for (uint32_t i = 0; i < JB_JOBS; i++) {
        pthread_mutex_lock(&jb.mu);
        while (jb.count == JB_QCAP) pthread_cond_wait(&jb.space, &jb.mu);
        jb.q[jb.tail] = i;
        jb.tail = (jb.tail + 1) % JB_QCAP;
        jb.count++;
        pthread_cond_signal(&jb.work);
        pthread_mutex_unlock(&jb.mu);
    }
    while (atomic_load(&jb.done) < JB_JOBS) sched_yield();
    pthread_mutex_lock(&jb.mu);
    jb.quit = 1;
    pthread_cond_broadcast(&jb.work);
    pthread_mutex_unlock(&jb.mu);
    for (int i = 0; i < JB_THREADS; i++) pthread_join(th[i], NULL);
    uint32_t done = atomic_load(&jb.done);
    if (done != JB_JOBS) bench_fail("trabajos");
    return fnv64(fnv64(BENCH_FNV0, atomic_load(&jb.result)), done);
}

// ------------------------------------------------------------------------------------------------ g) entero_saltos
// Juego: colisiones AABB con rejilla espacial (campos de bits, comparaciones, saltos) y busqueda A* con monton binario
// en un laberinto 128x128 (division y modulo por un divisor no constante -> UDIV, saltos condicionales, bitfields).
#define EN_OBJ 2000
#define EN_FRAMES 4000
#define EN_CELL 128
#define EN_GRID 80
typedef struct { int32_t x, y, w, h, vx, vy; uint32_t layer : 4, mask : 6, active : 1, kind : 5; } Obj;
static Obj objs[EN_OBJ];
static int32_t cell_head[EN_GRID * EN_GRID], obj_next[EN_OBJ];
static uint32_t g_div = 3;
#define MZ 128
static uint8_t maze[MZ * MZ];
static uint32_t gsc[MZ * MZ];
static uint64_t heap[MZ * MZ * 4];
static int heap_n;
static void hpush(uint64_t v) {
    int i = heap_n++;
    while (i > 0 && heap[(i - 1) / 2] > v) { heap[i] = heap[(i - 1) / 2]; i = (i - 1) / 2; }
    heap[i] = v;
}
static uint64_t hpop(void) {
    uint64_t top = heap[0], v = heap[--heap_n];
    int i = 0;
    for (;;) {
        int c = 2 * i + 1;
        if (c >= heap_n) break;
        if (c + 1 < heap_n && heap[c + 1] < heap[c]) c++;
        if (heap[c] >= v) break;
        heap[i] = heap[c];
        i = c;
    }
    if (heap_n) heap[i] = v;
    return top;
}
static int astar(int start, int goal, uint64_t *expanded) {
    for (int i = 0; i < MZ * MZ; i++) gsc[i] = 0xffffffffu;
    heap_n = 0;
    gsc[start] = 0;
    hpush((uint64_t)0 << 32 | (uint32_t)start);
    int gx = goal % MZ, gy = goal / MZ;
    static const int dx[4] = {1, -1, 0, 0}, dy[4] = {0, 0, 1, -1};
    while (heap_n) {
        uint64_t top = hpop();
        int cur = (int)(top & 0xffffffffu);
        uint32_t f = (uint32_t)(top >> 32);
        (*expanded)++;
        int cx = cur % MZ, cy = cur / MZ;
        if (cur == goal) return (int)gsc[cur];
        if (f > gsc[cur] + (uint32_t)(abs(cx - gx) + abs(cy - gy)) * 10 + 1000) continue;
        for (int d = 0; d < 4; d++) {
            int nx = cx + dx[d], ny = cy + dy[d];
            if ((unsigned)nx >= MZ || (unsigned)ny >= MZ) continue;
            int n = ny * MZ + nx;
            if (maze[n]) continue;
            uint32_t g = gsc[cur] + 10 + (uint32_t)(n * 2654435761u >> 7) % g_div;
            if (g < gsc[n]) {
                gsc[n] = g;
                uint32_t h = (uint32_t)(abs(nx - gx) + abs(ny - gy)) * 10;
                hpush((uint64_t)(g + h) << 32 | (uint32_t)n);
            }
        }
    }
    return -1;
}
static uint64_t b_entero_saltos(void) {
    uint64_t s = BENCH_SEED ^ 0xabc;
    g_div = 3 + (uint32_t)(xs(&s) & 1);
    for (int i = 0; i < EN_OBJ; i++) {
        Obj *o = &objs[i];
        o->w = 20 + (int32_t)(xs(&s) % 100); o->h = 20 + (int32_t)(xs(&s) % 100);
        o->x = (int32_t)(xs(&s) % 9000) + 400; o->y = (int32_t)(xs(&s) % 9000) + 400;
        o->vx = (int32_t)(xs(&s) % 41) - 20; o->vy = (int32_t)(xs(&s) % 41) - 20;
        o->layer = xs(&s) & 15; o->mask = xs(&s) & 63; o->active = 1; o->kind = xs(&s) & 31;
    }
    uint64_t pairs = 0, h = BENCH_FNV0;
    for (int f = 0; f < EN_FRAMES; f++) {
        for (int i = 0; i < EN_GRID * EN_GRID; i++) cell_head[i] = -1;
        for (int i = 0; i < EN_OBJ; i++) {
            Obj *o = &objs[i];
            o->x += o->vx; o->y += o->vy;
            if (o->x < 200 || o->x > 9800) o->vx = -o->vx;
            if (o->y < 200 || o->y > 9800) o->vy = -o->vy;
            int c = ((o->y + o->h / 2) / EN_CELL) * EN_GRID + (o->x + o->w / 2) / EN_CELL;
            obj_next[i] = cell_head[c];
            cell_head[c] = i;
        }
        for (int i = 0; i < EN_OBJ; i++) {
            Obj *a = &objs[i];
            int cx = (a->x + a->w / 2) / EN_CELL, cy = (a->y + a->h / 2) / EN_CELL;
            for (int yy = cy - 1; yy <= cy + 1; yy++)
                for (int xx = cx - 1; xx <= cx + 1; xx++) {
                    if ((unsigned)xx >= EN_GRID || (unsigned)yy >= EN_GRID) continue;
                    for (int j = cell_head[yy * EN_GRID + xx]; j >= 0; j = obj_next[j]) {
                        if (j <= i) continue;
                        Obj *b = &objs[j];
                        if (!((a->mask >> (b->layer & 3)) & 1) && !((b->mask >> (a->layer & 3)) & 1)) continue;
                        if (a->x < b->x + b->w && b->x < a->x + a->w && a->y < b->y + b->h && b->y < a->y + a->h) pairs += 1 + (a->kind ^ b->kind);
                    }
                }
        }
        h = fnv64(h, pairs);
    }
    uint64_t expanded = 0, paths = 0;
    for (int k = 0; k < 120; k++) {
        for (int i = 0; i < MZ * MZ; i++) maze[i] = (xs(&s) % 100) < 28;
        int a = (int)(xs(&s) % (MZ * MZ)), b = (int)(xs(&s) % (MZ * MZ));
        maze[a] = maze[b] = 0;
        int len = astar(a, b, &expanded);
        paths = paths * 1000003 + (uint64_t)(int64_t)len;
    }
    h = fnv64(h, pairs); h = fnv64(h, expanded);
    return fnv64(h, paths);
}

// ------------------------------------------------------------------------------------------------ h) malloc_churn
// Juego/il2cpp/Mono: millones de malloc/free de tamanos variados (HLE de malloc/free/memcpy), bloques vivos en
// una tabla de huecos y una lista enlazada de nodos pequenos que se recorre y libera por lotes.
#define MC_ITERS 8000000
#define MC_SLOTS 4096
typedef struct LNode { struct LNode *next; uint32_t size, chk; } LNode;
static void *mc_slot[MC_SLOTS];
static uint64_t b_malloc_churn(void) {
    uint64_t s = BENCH_SEED ^ 0xfeed, acc = 0;
    static uint8_t src[512];
    for (int i = 0; i < 512; i++) src[i] = (uint8_t)xs(&s);
    memset(mc_slot, 0, sizeof mc_slot);
    LNode *list = NULL;
    int ln = 0;
    for (int it = 0; it < MC_ITERS; it++) {
        uint64_t r = xs(&s);
        uint32_t idx = (uint32_t)(r >> 8) % MC_SLOTS;
        uint8_t *p = mc_slot[idx];
        if (p) {
            uint32_t size, chk;
            memcpy(&size, p, 4); memcpy(&chk, p + 4, 4);
            if (chk != (size * 2654435761u) ) bench_fail("malloc_churn");
            acc += size + p[8];
            free(p);
            mc_slot[idx] = NULL;
        } else {
            uint32_t size = 16 + (uint32_t)(r >> 40) % 2000;
            if ((r & 63) == 0) size = 4096 + (uint32_t)(r >> 24) % 60000;
            p = malloc(size);
            if (!p) { bench_fail("malloc_churn: sin memoria"); continue; }
            uint32_t chk = size * 2654435761u;
            memcpy(p, &size, 4); memcpy(p + 4, &chk, 4);
            uint32_t n = size - 8 < 512 ? size - 8 : 512;
            memcpy(p + 8, src, n);
            mc_slot[idx] = p;
        }
        if ((r & 7) == 0) {
            LNode *n = malloc(sizeof(LNode) + (r >> 50 & 31));
            n->next = list; n->size = (uint32_t)r; n->chk = ~n->size;
            list = n;
            ln++;
        }
        if (ln >= 256) {  // recorre y libera la lista
            for (LNode *q = list, *nx; q; q = nx) { nx = q->next; if (q->chk != ~q->size) bench_fail("malloc_churn lista"); acc += q->size & 0xff; free(q); }
            list = NULL;
            ln = 0;
        }
    }
    for (int i = 0; i < MC_SLOTS; i++) if (mc_slot[i]) { acc += ((uint8_t *)mc_slot[i])[8]; free(mc_slot[i]); mc_slot[i] = NULL; }
    for (LNode *q = list, *nx; q; q = nx) { nx = q->next; free(q); }
    return fnv64(BENCH_FNV0, acc);
}

// ------------------------------------------------------------------------------------------------ i) cadenas
// Juego: construccion de nombres/rutas/claves (snprintf con enteros, cadenas y double), strlen, concatenacion con memcpy,
// comparaciones memcmp/strcmp (HLE de libc).
#define CD_N 4000
#define CD_ROUNDS 400
static char cstr[CD_N][96];
static char cbig[1 << 20];
static uint64_t b_cadenas(void) {
    static const char *names[8] = {"jugador", "moneda", "tren", "obstaculo", "edificio", "particula", "sonido", "cuadro"};
    uint64_t s = BENCH_SEED ^ 0x55, h = BENCH_FNV0, total = 0, cmpacc = 0;
    size_t pos = 0;
    for (int r = 0; r < CD_ROUNDS; r++) {
        for (int i = 0; i < CD_N; i++) {
            snprintf(cstr[i], sizeof cstr[i], "obj_%u_%s_%08x_%.3f", (unsigned)i, names[i & 7], (unsigned)xs(&s), (double)i * 0.125 + r);
            total += strlen(cstr[i]);
        }
        for (int i = 0; i < CD_N; i++) {
            size_t n = strlen(cstr[i]);
            if (pos + n + 1 > sizeof cbig) { h = bench_fnv(h, cbig, pos); pos = 0; }
            memcpy(cbig + pos, cstr[i], n);
            pos += n;
            cbig[pos++] = ';';
            int j = (int)((uint32_t)(i * 7 + r) % CD_N);
            int c1 = memcmp(cstr[i], cstr[j], 12), c2 = strcmp(cstr[i], cstr[j]);
            cmpacc = cmpacc * 3 + (uint64_t)((c1 > 0) - (c1 < 0)) + (uint64_t)((c2 > 0) - (c2 < 0)) * 5;
        }
    }
    h = bench_fnv(h, cbig, pos);
    return fnv64(fnv64(h, total), cmpacc);
}

// ------------------------------------------------------------------------------------------------ j) senales_lockfree
// Estres: mientras 4 hilos usan la pila sin bloqueos (LL/SC), otro hilo les manda SIGUSR1 con pthread_kill unas 2 000
// veces por segundo (como la suspension del GC de Unity/Mono). El manejador incrementa un contador y escribe en otro
// granulo. Debe terminar sin cuelgue y con conteo exacto; el numero de senales va solo a BENCHINFO.
// En Android, ART bloquea SIGUSR1 (y SIGQUIT, SIGPIPE) en el zygote: el hilo "Signal Catcher" la atiende con sigwait y
// todo hilo de la app la hereda bloqueada (tambien los creados con pthread_create desde un hilo Java). Una senal
// enviada con pthread_kill a un hilo que la tiene bloqueada queda pendiente y nunca llega al manejador (igual en ARM
// real y en x86_64 nativo). Por eso cada hilo de trabajo la desbloquea explicitamente, como haria un juego que la use.
static _Atomic uint32_t sg_count, sg_sent, sg_finished, sg_release, sg_stop;
static volatile uint32_t sg_mem[16 * 32];
static pthread_t sg_th[4];
static void sg_handler(int sig) {
    (void)sig;
    uint32_t n = atomic_fetch_add(&sg_count, 1);
    sg_mem[(n & 15) * 32] = n;
}
static void *sg_worker(void *a) {
    sigset_t u;
    sigemptyset(&u);
    sigaddset(&u, SIGUSR1);
    pthread_sigmask(SIG_UNBLOCK, &u, NULL);
    st_work((int)(intptr_t)a);
    atomic_fetch_add(&sg_finished, 1);
    while (!atomic_load(&sg_release)) sched_yield();
    return NULL;
}
static void *sg_signaler(void *a) {
    (void)a;
    uint32_t k = 0;
    while (!atomic_load(&sg_stop)) {
        if (pthread_kill(sg_th[k++ & 3], SIGUSR1) == 0) atomic_fetch_add(&sg_sent, 1);
        struct timespec ts = {0, 500000};  // 2 000 por segundo
        nanosleep(&ts, NULL);
    }
    return NULL;
}
static uint64_t b_senales_lockfree(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = sg_handler;
    sa.sa_flags = SA_RESTART;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);
    sigset_t cur;
    pthread_sigmask(SIG_BLOCK, NULL, &cur);
    int heredada = sigismember(&cur, SIGUSR1) == 1;  // 1 en Android (ART), 0 en un proceso Linux normal
    g_iters = 600000;
    st_init(16);
    atomic_store(&sg_count, 0); atomic_store(&sg_sent, 0); atomic_store(&sg_finished, 0); atomic_store(&sg_release, 0); atomic_store(&sg_stop, 0);
    for (int i = 0; i < 4; i++) pthread_create(&sg_th[i], NULL, sg_worker, (void *)(intptr_t)i);
    pthread_t sig;
    pthread_create(&sig, NULL, sg_signaler, NULL);
    while (atomic_load(&sg_finished) < 4) sched_yield();
    atomic_store(&sg_stop, 1);
    pthread_join(sig, NULL);
    atomic_store(&sg_release, 1);
    for (int i = 0; i < 4; i++) pthread_join(sg_th[i], NULL);
    bench_info("senales_lockfree senales_entregadas=%u enviadas=%u sigusr1_bloqueada_al_entrar=%d", (unsigned)atomic_load(&sg_count),
               (unsigned)atomic_load(&sg_sent), heredada);
    return st_check(4, 16, "senales_lockfree");
}

// ------------------------------------------------------------------------------------------------ k) hilos_vida
// Unity/Android: hilos que nacen y mueren en rafagas (pools, descargas, audio) mientras otros hacen LDXR/STXR.
// 2 000 hilos en 40 rafagas de 50, con pila pequena; 2 hilos de fondo con contadores atomicos y CAS.
#define HV_BURSTS 100
#define HV_PER 50
static uint64_t hv_res[HV_BURSTS * HV_PER];
static _Atomic uint32_t hv_stop;
static _Atomic uint64_t hv_bg[2][16], hv_cas;
static void *hv_thread(void *a) {
    uint64_t id = (uint64_t)(intptr_t)a, h = id + 1, r = 0;
    for (int k = 0; k < 64; k++) { bench_xs(&h); r ^= h; }
    hv_res[id] = r;
    return NULL;
}
static void *hv_bg_thread(void *a) {
    int t = (int)(intptr_t)a;
    while (!atomic_load_explicit(&hv_stop, memory_order_relaxed)) {
        atomic_fetch_add(&hv_bg[t][0], 1);
        uint64_t old = atomic_load(&hv_cas);
        while (!atomic_compare_exchange_weak(&hv_cas, &old, old + 3)) {}
    }
    return NULL;
}
static uint64_t b_hilos_vida(void) {
    memset(hv_res, 0, sizeof hv_res);
    atomic_store(&hv_stop, 0);
    pthread_t bg[2];
    for (int i = 0; i < 2; i++) pthread_create(&bg[i], NULL, hv_bg_thread, (void *)(intptr_t)i);
    pthread_attr_t at_;
    pthread_attr_init(&at_);
    pthread_attr_setstacksize(&at_, 128 * 1024);
    int created = 0;
    for (int b = 0; b < HV_BURSTS; b++) {
        pthread_t th[HV_PER];
        int n = 0;
        for (int i = 0; i < HV_PER; i++)
            if (pthread_create(&th[n], &at_, hv_thread, (void *)(intptr_t)(b * HV_PER + i)) == 0) { n++; created++; }
        for (int i = 0; i < n; i++) pthread_join(th[i], NULL);
    }
    atomic_store(&hv_stop, 1);
    for (int i = 0; i < 2; i++) pthread_join(bg[i], NULL);
    pthread_attr_destroy(&at_);
    if (created != HV_BURSTS * HV_PER) bench_fail("hilos_vida");
    return fnv64(bench_fnv(BENCH_FNV0, hv_res, sizeof hv_res), (uint64_t)created);
}

// ------------------------------------------------------------------------------------------------ bateria
static const bench_def LIST[] = {
    {"mat4", b_mat4},
    {"fisica", b_fisica},
    {"particulas_mem", b_particulas_mem},
    {"pila_lockfree", b_pila_lockfree},
    {"atomicos", b_atomicos},
#if defined(__aarch64__)
    {"atomicos_lse", b_atomicos_lse},
#endif
    {"trabajos", b_trabajos},
    {"entero_saltos", b_entero_saltos},
    {"malloc_churn", b_malloc_churn},
    {"cadenas", b_cadenas},
    {"senales_lockfree", b_senales_lockfree},
    {"hilos_vida", b_hilos_vida},
};
#define NLIST ((int)(sizeof LIST / sizeof LIST[0]))

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

int bench_run(int rep, bench_emit emit, const bench_def *extra, int nextra, const char *only) {
    g_emit = emit;
    g_ok = 1;
    uint64_t first[64];
    int have[64] = {0};
    int64_t t_all = now_ms();
    char line[200];
    for (int r = 1; r <= rep; r++) {
        for (int i = 0; i < NLIST + nextra; i++) {
            const bench_def *d = i < NLIST ? &LIST[i] : &extra[i - NLIST];
            if (only && !strstr(d->name, only)) continue;
            if (g_begin) g_begin(d->name);  // fuera de la medida
            int64_t t0 = now_ms();
            uint64_t sum = d->fn();
            int64_t ms = now_ms() - t0;
            snprintf(line, sizeof line, "BENCH %s rep=%d ms=%lld sum=%016llx", d->name, r, (long long)ms, (unsigned long long)sum);
            emit(line);
            if (!have[i]) { have[i] = 1; first[i] = sum; }
            else if (first[i] != sum) { g_ok = 0; bench_info("%s checksum distinto entre repeticiones", d->name); }
        }
    }
    snprintf(line, sizeof line, "BENCH fin total_ms=%lld ok=%d", (long long)(now_ms() - t_all), g_ok);
    emit(line);
    return g_ok;
}
