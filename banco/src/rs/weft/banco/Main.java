package rs.weft.banco;

import android.app.Activity;
import android.app.ActivityManager;
import android.app.ApplicationExitInfo;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioTrack;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.os.Process;
import android.os.RemoteException;
import android.util.Log;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * Banco de pruebas: cada prueba hace algo real (calcular, dibujar, sonar), se comprueba sola y deja una linea
 * "BANCO nombre=OK detalle" o "BANCO nombre=FALLO detalle" en el registro. La biblioteca nativa se compila para una
 * sola arquitectura por paquete: x86_64 (nativa) o arm64 (solo funciona con un traductor de ARM instalado).
 */
public class Main extends Activity {
    static final String TAG = "banco";
    static native String abi();
    static native String calculo();
    static native String hilos();
    static native String opengl();
    static native String vulkan();
    static native String fallos();
    static native String codigo();
    static native String mascara();
    /** registrado en JNI_OnLoad con RegisterNatives (no por nombre) */
    static native String mezcla(int a, long b, float f, double d, int e, float g, double h, long i, float j, double k, int l,
                                float m, double n, long o, float p, double q, int r, double s);
    static native int bench(int rep, String solo);
    static native int pantalla(android.view.Surface s, int faseS, int ini);
    static Main inst;
    volatile String current = "";

    final List<String> lines = new ArrayList<>();
    Panel panel;
    final Handler ui = new Handler(Looper.getMainLooper());

    void report(String name, String result) {
        String line = name + "=" + result;
        Log.i(TAG, "BANCO " + line);
        synchronized (lines) {
            lines.add(line);
        }
        ui.post(() -> panel.invalidate());
    }

    interface Test {
        String run() throws Throwable;
    }

    void test(String name, Test t) {
        try {
            report(name, t.run());
        } catch (Throwable e) {
            report(name, "FALLO " + e);
        }
    }

    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        if ("gles_pantalla".equals(getIntent().getStringExtra("modo"))) {
            final int fase = getIntent().getIntExtra("fase", 30);
            android.view.SurfaceView sv = new android.view.SurfaceView(this);
            setContentView(sv);
            sv.getHolder().addCallback(new android.view.SurfaceHolder.Callback() {
                boolean started;
                public void surfaceCreated(android.view.SurfaceHolder h) {}
                public void surfaceChanged(android.view.SurfaceHolder h, int f, int w, int hh) {
                    if (started) return;
                    started = true;
                    final android.view.Surface sf = h.getSurface();
                    new Thread(() -> {
                        try {
                            System.loadLibrary("banco");
                            int r = pantalla(sf, fase, getIntent().getIntExtra("ini", 0));
                            Log.i(TAG, "PANTALLA retorno=" + r);
                        } catch (Throwable e) {
                            Log.e(TAG, "PANTALLA error " + e);
                        }
                    }, "pantalla").start();
                }
                public void surfaceDestroyed(android.view.SurfaceHolder h) {}
            });
            return;
        }
        panel = new Panel(this);
        setContentView(panel);
        panel.setFocusableInTouchMode(true);
        panel.requestFocus();
        inst = this;
        if ("bench".equals(getIntent().getStringExtra("modo"))) {
            final int rep = getIntent().getIntExtra("rep", 1);
            final String solo = getIntent().getStringExtra("solo");  // opcional: solo las cargas cuyo nombre lo contiene
            new Thread(() -> runBench(rep, solo), "bench").start();
        } else if ("caso".equals(getIntent().getStringExtra("modo"))) {
            // un solo caso del proceso ":bajo" (p. ej. vulkan con otra configuracion del traductor) y "fin"
            final String caso = getIntent().getStringExtra("caso");
            new Thread(() -> {
                report(caso == null ? "caso" : caso, enProceso(caso));
                report("fin", "OK");
            }, "banco").start();
        } else {
            new Thread(this::runAll, "banco").start();
        }
    }

    /** Modo bench: sin pruebas funcionales; la bateria nativa deja las lineas BENCH en el registro y aqui en pantalla. */
    void runBench(int rep, String solo) {
        try {
            System.loadLibrary("banco");
            bench(rep, solo);
        } catch (Throwable e) {
            Log.e(TAG, "BENCH fin total_ms=0 ok=0 error=" + e);
            progress("fin total_ms=0 ok=0 error=" + e);
        }
    }

    /** Llamado desde el codigo nativo entre cargas (no dentro): "ejecutando: x ..." o una linea de resultado. */
    static void progress(String s) {
        final Main m = inst;
        if (m == null) return;
        if (s.startsWith("ejecutando:")) {
            m.current = s;
        } else {
            m.current = "";
            synchronized (m.lines) {
                m.lines.add(s);
                while (m.lines.size() > 24) m.lines.remove(0);
            }
        }
        m.ui.post(() -> m.panel.invalidate());
    }

    void runAll() {
        test("biblioteca", () -> {
            System.loadLibrary("banco");
            return "OK " + abi();
        });
        test("calculo_java", Main::calculoJava);
        test("calculo_nativo", Main::calculo);
        test("hilos_nativos", Main::hilos);
        test("dibujo_2d", Main::dibujo2d);
        test("opengl", Main::opengl);
        // Vulkan en el proceso ":bajo": con un traductor de ARM que lo rompa, muere ese proceso y no la app
        report("vulkan", enProceso("vulkan"));
        test("archivos", this::archivos);
        test("sensores", this::sensores);
        test("red", Main::red);
        test("sonido", Main::sonido);
        // al final y en otro proceso (servicio Bajo, ":bajo"): con un traductor que no los soporte, ese proceso puede morir;
        // este sigue y anota el caso como CAIDO
        for (String caso : BAJO) report(caso, enProceso(caso));
        report("fin", "OK");
    }

    static final String[] BAJO = {"fallo_recuperable", "codigo_propio", "mascara_senales", "jni_registrado"};

    /**
     * Ejecuta un caso de bajo nivel en el proceso ":bajo" y devuelve su resultado. Si el proceso muere: "CAIDO senal=N
     * motivo=M" (de ApplicationExitInfo, Android 11 o mas nuevo; si no, senal=?); si no responde en 30 s (60 s vulkan: dibuja
     * por software y puede pasar por un traductor): FALLO y se mata.
     */
    String enProceso(String caso) {
        final CountDownLatch conectado = new CountDownLatch(1), fin = new CountDownLatch(1);
        final Messenger[] servicio = new Messenger[1];
        final String[] resultado = new String[1];
        final int[] pid = {0};
        final boolean[] murio = {false};
        HandlerThread respuestas = new HandlerThread("bajo-respuestas");
        respuestas.start();
        Messenger yo = new Messenger(new Handler(respuestas.getLooper()) {
            @Override
            public void handleMessage(Message m) {
                if (m.what == Bajo.PID) {
                    pid[0] = m.arg1;
                } else if (m.what == Bajo.RESULTADO) {
                    resultado[0] = m.getData().getString("r");
                    fin.countDown();
                }
            }
        });
        ServiceConnection con = new ServiceConnection() {
            public void onServiceConnected(ComponentName n, IBinder b) {
                try {
                    b.linkToDeath(() -> {
                        murio[0] = true;
                        fin.countDown();
                    }, 0);
                } catch (RemoteException e) {
                    murio[0] = true;
                    fin.countDown();
                }
                servicio[0] = new Messenger(b);
                conectado.countDown();
            }

            public void onServiceDisconnected(ComponentName n) {
                murio[0] = true;
                fin.countDown();
            }
        };
        try {
            if (!bindService(new Intent(this, Bajo.class), con, Context.BIND_AUTO_CREATE)) return "FALLO no se pudo enlazar el servicio";
            if (!conectado.await(20, TimeUnit.SECONDS)) return "FALLO el servicio no arranco";
            Message m = Message.obtain(null, Bajo.CASO);
            Bundle d = new Bundle();
            d.putString("caso", caso);
            m.setData(d);
            m.replyTo = yo;
            servicio[0].send(m);
            boolean termino = fin.await("vulkan".equals(caso) ? 60 : 30, TimeUnit.SECONDS);
            if (resultado[0] != null) return resultado[0];
            if (!termino) {
                if (pid[0] > 0) Process.killProcess(pid[0]);
                return "FALLO tiempo_agotado pid=" + pid[0];
            }
            return caido(pid[0]);
        } catch (Throwable e) {
            return murio[0] ? caido(pid[0]) : "FALLO " + e;
        } finally {
            try {
                unbindService(con);
            } catch (Throwable e) {
                // ya no estaba enlazado
            }
            respuestas.quitSafely();
        }
    }

    /** Linea de un caso cuyo proceso murio: la senal y el motivo segun Android, si los registro (lo hace tras unos segundos). */
    String caido(int pid) {
        String senal = "?", motivo = "?";
        if (Build.VERSION.SDK_INT >= 30 && pid > 0) {
            ActivityManager am = (ActivityManager) getSystemService(Context.ACTIVITY_SERVICE);
            for (int i = 0; i < 40; i++) {
                List<ApplicationExitInfo> l = am.getHistoricalProcessExitReasons(getPackageName(), pid, 1);
                if (!l.isEmpty()) {
                    ApplicationExitInfo x = l.get(0);
                    int r = x.getReason();
                    motivo = r == ApplicationExitInfo.REASON_CRASH_NATIVE ? "crash_nativo" : r == ApplicationExitInfo.REASON_SIGNALED ? "senal"
                            : r == ApplicationExitInfo.REASON_CRASH ? "crash_java" : "motivo_" + r;
                    if (r == ApplicationExitInfo.REASON_CRASH_NATIVE || r == ApplicationExitInfo.REASON_SIGNALED) senal = String.valueOf(x.getStatus());
                    break;
                }
                try {
                    Thread.sleep(250);
                } catch (InterruptedException e) {
                    break;
                }
            }
        }
        return "CAIDO senal=" + senal + " motivo=" + motivo + " pid=" + pid;
    }

    static String calculoJava() {
        // criba de primos y una suma de comprobacion
        int n = 200000, count = 0;
        boolean[] comp = new boolean[n + 1];
        long sum = 0;
        for (int i = 2; i <= n; i++) {
            if (!comp[i]) {
                count++;
                sum += i;
                for (long j = (long) i * i; j <= n; j += i) comp[(int) j] = true;
            }
        }
        double x = 0;
        for (int i = 1; i <= 1000; i++) x += Math.sqrt(i) * Math.sin(i);
        boolean ok = count == 17984 && sum == 1709600813L && Math.abs(x - (-2.5879)) < 0.01;
        return (ok ? "OK" : "FALLO") + " primos=" + count + " suma=" + sum + " x=" + String.format("%.4f", x);
    }

    static String dibujo2d() {
        Bitmap bmp = Bitmap.createBitmap(200, 200, Bitmap.Config.ARGB_8888);
        Canvas c = new Canvas(bmp);
        c.drawColor(Color.rgb(10, 20, 30));
        Paint p = new Paint();
        p.setColor(Color.rgb(200, 0, 0));
        c.drawRect(20, 20, 100, 100, p);
        p.setColor(Color.rgb(0, 180, 0));
        p.setAntiAlias(true);
        c.drawCircle(150, 150, 30, p);
        p.setColor(Color.WHITE);
        p.setTextSize(30);
        c.drawText("2D", 120, 60, p);
        int bg = bmp.getPixel(5, 5), rect = bmp.getPixel(60, 60), circ = bmp.getPixel(150, 150);
        int white = 0;
        for (int y = 30; y < 65; y++) for (int x = 118; x < 170; x++) if (bmp.getPixel(x, y) == Color.WHITE) white++;
        boolean ok = bg == Color.rgb(10, 20, 30) && rect == Color.rgb(200, 0, 0) && circ == Color.rgb(0, 180, 0) && white > 40;
        return (ok ? "OK" : "FALLO") + " fondo=" + Integer.toHexString(bg) + " rect=" + Integer.toHexString(rect) + " circulo=" + Integer.toHexString(circ) + " texto_px=" + white;
    }

    String archivos() throws Exception {
        File f = new File(getFilesDir(), "prueba.bin");
        byte[] data = new byte[1 << 20];
        for (int i = 0; i < data.length; i++) data[i] = (byte) (i * 31 + 7);
        try (FileOutputStream o = new FileOutputStream(f)) {
            o.write(data);
        }
        byte[] back = new byte[data.length];
        int n;
        try (FileInputStream in = new FileInputStream(f)) {
            n = in.read(back);
        }
        boolean ok = n == data.length && java.util.Arrays.equals(data, back);
        f.delete();
        return (ok ? "OK" : "FALLO") + " bytes=" + n;
    }

    String sensores() throws Exception {
        SensorManager sm = (SensorManager) getSystemService(Context.SENSOR_SERVICE);
        Sensor acc = sm.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
        if (acc == null) return "FALLO sin acelerometro";
        final float[] v = new float[3];
        final CountDownLatch got = new CountDownLatch(1);
        SensorEventListener l = new SensorEventListener() {
            public void onSensorChanged(SensorEvent e) {
                System.arraycopy(e.values, 0, v, 0, 3);
                got.countDown();
            }

            public void onAccuracyChanged(Sensor s, int a) {}
        };
        sm.registerListener(l, acc, SensorManager.SENSOR_DELAY_NORMAL, ui);
        boolean ok = got.await(8, TimeUnit.SECONDS);
        sm.unregisterListener(l);
        double g = Math.sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        return (ok && Math.abs(g - 9.81) < 0.5 ? "OK" : "FALLO") + String.format(" acelerometro=%.2f,%.2f,%.2f", v[0], v[1], v[2]);
    }

    static String red() {
        try {
            HttpURLConnection c = (HttpURLConnection) new URL("http://example.com/").openConnection();
            c.setConnectTimeout(8000);
            c.setReadTimeout(8000);
            int code = c.getResponseCode();
            return (code == 200 ? "OK" : "FALLO") + " http=" + code;
        } catch (Exception e) {
            return "FALLO " + e.getClass().getSimpleName() + ": " + e.getMessage();
        }
    }

    /** Tono de 1000 Hz durante 2 s: el emulador lo graba y la prueba mide su frecuencia desde fuera. */
    static String sonido() throws Exception {
        int rate = 48000, n = rate * 2;
        short[] pcm = new short[n];
        for (int i = 0; i < n; i++) pcm[i] = (short) (Math.sin(2 * Math.PI * 1000 * i / rate) * 20000);
        AudioTrack t = new AudioTrack(AudioManager.STREAM_MUSIC, rate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT, n * 2, AudioTrack.MODE_STATIC);
        t.write(pcm, 0, n);
        t.setVolume(1.0f);
        t.play();
        Thread.sleep(2300);
        int pos = t.getPlaybackHeadPosition();
        t.release();
        return (pos >= n * 9 / 10 ? "OK" : "FALLO") + " muestras_reproducidas=" + pos + " tono_hz=1000";
    }

    /** Pantalla: resultados y eco de la entrada (toques y teclas), que tambien va al registro. */
    class Panel extends View {
        final Paint p = new Paint();
        float tx = -1, ty = -1;

        Panel(Context c) {
            super(c);
            setFocusable(true);
        }

        @Override
        protected void onDraw(Canvas c) {
            c.drawColor(Color.rgb(16, 24, 40));
            p.setAntiAlias(true);
            p.setColor(Color.WHITE);
            p.setTextSize(34);
            c.drawText("Banco de pruebas weft", 20, 60, p);
            p.setTextSize(24);
            int y = 110;
            synchronized (lines) {
                for (String l : lines) {
                    p.setColor(l.contains("=OK") || l.contains(" rep=") || l.startsWith("fin total_ms") && l.contains("ok=1") ? Color.rgb(120, 230, 140) : Color.rgb(255, 120, 120));
                    c.drawText(l.length() > 54 ? l.substring(0, 54) : l, 20, y, p);
                    y += 34;
                }
            }
            if (!current.isEmpty()) {
                p.setColor(Color.YELLOW);
                c.drawText(current, 20, y, p);
            }
            // muestra 2D visible: figuras de colores
            p.setColor(Color.rgb(230, 60, 60));
            c.drawRect(40, getHeight() - 260, 200, getHeight() - 100, p);
            p.setColor(Color.rgb(60, 200, 90));
            c.drawCircle(330, getHeight() - 180, 80, p);
            p.setColor(Color.rgb(70, 130, 240));
            c.drawRoundRect(460, getHeight() - 260, 680, getHeight() - 100, 30, 30, p);
            if (tx >= 0) {
                p.setColor(Color.YELLOW);
                c.drawCircle(tx, ty, 26, p);
            }
        }

        @Override
        public boolean onTouchEvent(MotionEvent e) {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN || e.getActionMasked() == MotionEvent.ACTION_UP) {
                tx = e.getX();
                ty = e.getY();
                String a = e.getActionMasked() == MotionEvent.ACTION_DOWN ? "baja" : "sube";
                Log.i(TAG, String.format("BANCO toque=%s x=%.3f y=%.3f fuente=%s", a, e.getRawX() / getRootView().getWidth(), e.getRawY() / getRootView().getHeight(),
                        e.getSource() == android.view.InputDevice.SOURCE_TOUCHSCREEN ? "tactil" : "otra"));
                invalidate();
            }
            return true;
        }

        @Override
        public boolean onKeyDown(int code, KeyEvent e) {
            Log.i(TAG, "BANCO tecla=" + KeyEvent.keyCodeToString(code) + " shift=" + e.isShiftPressed());
            return true;
        }
    }
}
