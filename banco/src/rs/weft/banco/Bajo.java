package rs.weft.banco;

import android.app.Service;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Message;
import android.os.Messenger;
import android.os.Process;
import android.os.RemoteException;
import android.util.Log;

/**
 * Casos de bajo nivel (fallo recuperable, codigo propio, mascara de senales, JNI registrado) y Vulkan en un proceso aparte
 * (":bajo", ver AndroidManifest.xml): si un traductor de ARM los rompe y el proceso muere, el proceso principal sigue
 * con sus mediciones y anota el caso como CAIDO. Protocolo por Messenger: la actividad manda un mensaje CASO con el
 * nombre del caso y su Messenger en replyTo; el servicio responde PID (arg1 = su pid) antes de empezar y RESULTADO
 * (Bundle "r") al terminar. Los metodos nativos son los de Main (mismos simbolos JNI).
 */
public class Bajo extends Service {
    static final int CASO = 1, PID = 2, RESULTADO = 3;
    HandlerThread hilo;
    Messenger entrada;

    @Override
    public void onCreate() {
        super.onCreate();
        // los casos corren fuera del hilo principal del proceso
        hilo = new HandlerThread("bajo");
        hilo.start();
        entrada = new Messenger(new Handler(hilo.getLooper()) {
            @Override
            public void handleMessage(Message m) {
                if (m.what != CASO || m.replyTo == null) return;
                String caso = m.getData().getString("caso");
                try {
                    m.replyTo.send(Message.obtain(null, PID, Process.myPid(), 0));
                    Log.i(Main.TAG, "bajo: " + caso + " en el proceso " + Process.myPid());
                    Message r = Message.obtain(null, RESULTADO);
                    Bundle d = new Bundle();
                    d.putString("r", ejecutar(caso));
                    r.setData(d);
                    m.replyTo.send(r);
                } catch (RemoteException e) {
                    Log.w(Main.TAG, "bajo: la actividad ya no escucha: " + e);
                }
            }
        });
    }

    @Override
    public IBinder onBind(Intent i) {
        return entrada.getBinder();
    }

    @Override
    public void onDestroy() {
        hilo.quitSafely();
        super.onDestroy();
    }

    static String ejecutar(String caso) {
        try {
            System.loadLibrary("banco");
            switch (caso == null ? "" : caso) {
                case "fallo_recuperable":
                    return Main.fallos();
                case "codigo_propio":
                    return Main.codigo();
                case "mascara_senales":
                    return Main.mascara();
                case "vulkan":
                    return Main.vulkan();
                case "jni_registrado":
                    return Main.mezcla(1, 2000000000000L, 3.5f, 4.25, -5, 6.5f, 7.125, -8L, 9.5f, 10.75, 11, 12.5f, 13.25, 14L,
                            15.5f, 16.125, 17, 18.5);
                default:
                    return "FALLO caso desconocido: " + caso;
            }
        } catch (Throwable e) {
            return "FALLO " + e;
        }
    }
}
