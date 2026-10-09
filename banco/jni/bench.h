// Banco de rendimiento: cargas nativas deterministas (semilla y trabajo fijos). C portable: el compilador genera
// FP/SIMD por elemento, LL/SC, etc. Sin JNI ni GLES; lo usan banco.c (APK) y bench_main.c (ejecutable).
#ifndef BENCH_H
#define BENCH_H
#include <stdint.h>

typedef uint64_t (*bench_fn)(void);
typedef struct {
    const char *name;
    bench_fn fn;
} bench_def;
typedef void (*bench_emit)(const char *line);

#define BENCH_SEED 0x9E3779B97F4A7C15ULL
#define BENCH_FNV0 0xcbf29ce484222325ULL

uint64_t bench_xs(uint64_t *s);                                      // xorshift64
uint64_t bench_fnv(uint64_t h, const void *p, uint64_t n);           // FNV-1a de 64 bits
void bench_m4_mul(float *o, const float *a, const float *b);         // 4x4 columna-mayor
void bench_fail(const char *what);                                   // marca ok=0
void bench_info(const char *fmt, ...);                               // linea "BENCHINFO ..."

// Aviso opcional (fuera de la medida) antes de cada carga, para mostrar progreso.
void bench_set_begin(void (*f)(const char *name));

// Ejecuta la bateria rep veces (mas las cargas extra) y emite las lineas BENCH. Devuelve 1 si todo coincide.
// only: si no es NULL, solo las cargas cuyo nombre lo contiene.
int bench_run(int rep, bench_emit emit, const bench_def *extra, int nextra, const char *only);
#endif
