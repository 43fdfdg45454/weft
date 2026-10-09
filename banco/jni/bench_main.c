// Ejecutable arm64 de la bateria (sin JNI ni GLES): bench-arm64 [rep] [filtro]
#include <stdio.h>
#include <stdlib.h>
#include "bench.h"

static void out(const char *line) {
    puts(line);
    fflush(stdout);
}

int main(int argc, char **argv) {
    int rep = argc > 1 ? atoi(argv[1]) : 1;
    return bench_run(rep < 1 ? 1 : rep, out, NULL, 0, argc > 2 ? argv[2] : NULL) ? 0 : 1;
}

// Entrada para "weft libbench.so bench_entry [rep]" (biblioteca compartida; weft no carga ejecutables de bionic).
int bench_entry(long rep) { return bench_run(rep < 1 ? 1 : (int)rep, out, NULL, 0, NULL) ? 0 : 1; }
