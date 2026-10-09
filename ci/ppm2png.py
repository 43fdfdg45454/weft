#!/usr/bin/env python3
"""Convierte una imagen PPM (P6) en PNG sin dependencias y comprueba que no es lisa.
Uso: ppm2png.py ENTRADA.ppm SALIDA.png  -> imprime "ANCHOxALTO colores=N"; sale con 1 si hay menos de 20 colores distintos.
     ppm2png.py --brillo ENTRADA.ppm FILAS -> imprime el brillo medio (0..255, luma BT.601) de las primeras FILAS filas."""
import struct
import sys
import zlib


def leer_ppm(ruta):
    d = open(ruta, "rb").read()
    campos, i = [], 0
    while len(campos) < 4:
        while d[i:i + 1].isspace():
            i += 1
        j = i
        while not d[j:j + 1].isspace():
            j += 1
        campos.append(d[i:j])
        i = j
    if campos[0] != b"P6" or campos[3] != b"255":
        raise SystemExit("no es un PPM P6 de 8 bits")
    return int(campos[1]), int(campos[2]), d[i + 1:]


def escribir_png(ruta, w, h, rgb):
    filas = b"".join(b"\x00" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def trozo(tipo, datos):
        c = struct.pack(">I", len(datos)) + tipo + datos
        return c + struct.pack(">I", zlib.crc32(tipo + datos) & 0xFFFFFFFF)

    with open(ruta, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + trozo(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + trozo(b"IDAT", zlib.compress(filas, 6)) + trozo(b"IEND", b""))


if len(sys.argv) == 4 and sys.argv[1] == "--brillo":
    w, h, rgb = leer_ppm(sys.argv[2])
    n = min(int(sys.argv[3]), h) * w
    if n <= 0:
        raise SystemExit("sin filas")
    print(round(sum(299 * rgb[i] + 587 * rgb[i + 1] + 114 * rgb[i + 2] for i in range(0, n * 3, 3)) / (1000 * n)))
    sys.exit(0)
if len(sys.argv) != 3:
    raise SystemExit(__doc__)
w, h, rgb = leer_ppm(sys.argv[1])
if len(rgb) < w * h * 3:
    raise SystemExit("PPM truncado")
colores = len({rgb[i:i + 3] for i in range(0, w * h * 3, 3)})
escribir_png(sys.argv[2], w, h, rgb[:w * h * 3])
print("%dx%d colores=%d" % (w, h, colores))
sys.exit(0 if colores >= 20 else 1)
