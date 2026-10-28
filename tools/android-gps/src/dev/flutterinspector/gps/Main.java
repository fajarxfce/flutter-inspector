package dev.flutterinspector.gps;

import android.content.Context;
import android.location.Criteria;
import android.location.Location;
import android.location.LocationListener;
import android.location.LocationManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Looper;
import android.os.SystemClock;
import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStreamReader;
import java.nio.channels.FileChannel;
import java.nio.channels.FileLock;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.util.concurrent.TimeUnit;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** A shell-UID mock provider with a host lease. No APK, root, or Flutter dependency. */
public final class Main {
    private static final File STATE = new File("/data/local/tmp/flutter-inspector-gps-state.json");
    private static final File LOCK = new File("/data/local/tmp/flutter-inspector-gps.lock");
    private static final String[] PROVIDERS = {"gps", "network"};
    private final LocationManager locations;
    private volatile long lastCommand = SystemClock.elapsedRealtime();
    private boolean active;
    private volatile boolean closed;

    private Main(LocationManager locations) { this.locations = locations; }

    public static void main(String[] args) {
        int code = 0;
        try {
            if (Build.VERSION.SDK_INT < 26) throw new IllegalStateException("GPS simulation needs Android 8 or newer");
            Looper.prepareMainLooper();
            Class<?> activityThread = Class.forName("android.app.ActivityThread");
            Object thread = activityThread.getMethod("systemMain").invoke(null);
            Context system = (Context) activityThread.getMethod("getSystemContext").invoke(thread);
            Context shell = system.createPackageContext("com.android.shell", 0);
            Main session = new Main((LocationManager) shell.getSystemService(Context.LOCATION_SERVICE));
            String mode = args.length == 0 ? "" : args[0];
            if ("--probe".equals(mode)) {
                emit(new JSONObject().put("ok", true).put("protocol", 1).put("sdk", Build.VERSION.SDK_INT)
                    .put("providers", new JSONArray(session.locations.getAllProviders()))
                    .put("locationEnabled", session.locationEnabled()).put("recoveryNeeded", STATE.exists()));
            } else if ("--watch".equals(mode)) {
                session.watch();
            } else {
                try (FileChannel channel = FileChannel.open(LOCK.toPath(), StandardOpenOption.CREATE, StandardOpenOption.WRITE);
                     FileLock lock = channel.tryLock()) {
                    if (lock == null) throw new IllegalStateException("Another Inspector GPS session owns this device; stop it first");
                    session.restore();
                    if ("--restore".equals(mode)) {
                        emit(new JSONObject().put("ok", true).put("stopped", true));
                    } else {
                        session.serve();
                    }
                }
            }
        } catch (Throwable error) {
            code = 1;
            failure(error);
        }
        System.exit(code);
    }

    private boolean locationEnabled() {
        return Build.VERSION.SDK_INT >= 28 ? locations.isLocationEnabled()
            : locations.isProviderEnabled(LocationManager.GPS_PROVIDER);
    }

    private void serve() throws Exception {
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            try { close(); } catch (Exception error) { failure(error); }
        }, "gps-cleanup"));
        Thread lease = new Thread(() -> {
            try {
                while (!closed) {
                    Thread.sleep(1000);
                    if (SystemClock.elapsedRealtime() - lastCommand > 10_000) {
                        close();
                        System.exit(0);
                    }
                }
            } catch (Throwable error) { failure(error); System.exit(1); }
        }, "gps-lease");
        lease.setDaemon(true);
        lease.start();
        emit(new JSONObject().put("ready", true).put("protocol", 1));
        try (BufferedReader input = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8))) {
            String line;
            while ((line = boundedLine(input)) != null) {
                lastCommand = SystemClock.elapsedRealtime();
                JSONObject request = new JSONObject(line);
                String op = request.getString("op");
                if ("stop".equals(op)) { close(); emit(new JSONObject().put("ok", true).put("stopped", true)); return; }
                if (!"fix".equals(op)) throw new IllegalArgumentException("Unknown GPS operation");
                JSONObject fix = request.getJSONObject("fix");
                apply(fix);
                emit(new JSONObject().put("ok", true).put("fix", fix));
            }
        } finally { close(); }
    }

    private static String boundedLine(BufferedReader reader) throws Exception {
        StringBuilder text = new StringBuilder();
        for (int ch; (ch = reader.read()) != -1;) {
            if (ch == '\n') return text.toString();
            if (text.length() >= 4096) throw new IllegalArgumentException("GPS command exceeds 4 KiB");
            text.append((char) ch);
        }
        return text.length() == 0 ? null : text.toString();
    }

    @SuppressWarnings("deprecation")
    private synchronized void apply(JSONObject fix) throws Exception {
        double latitude = number(fix, "latitude", -90, 90);
        double longitude = number(fix, "longitude", -180, 180);
        double altitude = number(fix, "altitude", -12000, 100000);
        float accuracy = (float) number(fix, "accuracy", 0.1, 10000);
        float speed = (float) number(fix, "speed", 0, 400);
        float heading = (float) number(fix, "heading", 0, 360);
        if (closed) throw new IllegalStateException("GPS session has stopped");
        if (!active) {
            if (!locationEnabled()) throw new IllegalStateException("Turn on Location on the Android device before starting GPS simulation");
            String mode = appOpMode(command("appops", "get", "com.android.shell", "android:mock_location"));
            JSONObject state = new JSONObject().put("mode", mode);
            File temporary = new File(STATE + ".tmp");
            try (FileOutputStream output = new FileOutputStream(temporary)) {
                output.write(state.toString().getBytes(StandardCharsets.UTF_8));
                output.getFD().sync();
            }
            Files.move(temporary.toPath(), STATE.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
            command("appops", "set", "com.android.shell", "android:mock_location", "allow");
            for (String provider : PROVIDERS) {
                locations.addTestProvider(provider, false, "gps".equals(provider), false, false,
                    true, true, true, Criteria.POWER_LOW, Criteria.ACCURACY_FINE);
                locations.setTestProviderEnabled(provider, true);
            }
            active = true;
        }
        for (String provider : PROVIDERS) {
            Location location = new Location(provider);
            location.setLatitude(latitude);
            location.setLongitude(longitude);
            location.setAltitude(altitude);
            location.setAccuracy(accuracy);
            location.setSpeed(speed);
            location.setBearing(heading);
            location.setTime(System.currentTimeMillis());
            location.setElapsedRealtimeNanos(SystemClock.elapsedRealtimeNanos());
            location.setVerticalAccuracyMeters(accuracy);
            location.setSpeedAccuracyMetersPerSecond(0.1f);
            location.setBearingAccuracyDegrees(1.0f);
            locations.setTestProviderLocation(provider, location);
        }
    }

    private synchronized void close() throws Exception {
        if (!closed) { closed = true; restore(); }
    }

    private void restore() throws Exception {
        if (!STATE.isFile()) return;
        JSONObject state = new JSONObject(new String(Files.readAllBytes(STATE.toPath()), StandardCharsets.UTF_8));
        // The journal survives a killed host/helper, allowing an explicit Restore GPS.
        command("appops", "set", "com.android.shell", "android:mock_location", "allow");
        for (String provider : PROVIDERS) locations.removeTestProvider(provider);
        command("appops", "set", "com.android.shell", "android:mock_location", state.getString("mode"));
        Files.delete(STATE.toPath());
        active = false;
    }

    private static double number(JSONObject value, String key, double min, double max) throws Exception {
        double number = value.getDouble(key);
        if (!Double.isFinite(number) || number < min || number > max)
            throw new IllegalArgumentException("Invalid GPS " + key);
        return number;
    }

    private static String appOpMode(String text) {
        if (text.contains("No operations.")) return "default";
        Matcher match = Pattern.compile("(?i)MOCK_LOCATION:\\s*(allow|ignore|deny|default|foreground)").matcher(text);
        if (!match.find()) throw new IllegalStateException("Cannot read existing mock-location permission; no settings changed");
        return match.group(1).toLowerCase(java.util.Locale.ROOT);
    }

    private static String command(String... args) throws Exception {
        String[] invocation = new String[args.length + 1];
        invocation[0] = "/system/bin/cmd";
        System.arraycopy(args, 0, invocation, 1, args.length);
        Process process = new ProcessBuilder(invocation).redirectErrorStream(true).start();
        if (!process.waitFor(3, TimeUnit.SECONDS)) { process.destroyForcibly(); throw new IllegalStateException("Android permission command timed out"); }
        byte[] buffer = new byte[16384];
        int length = process.getInputStream().read(buffer);
        String output = length < 0 ? "" : new String(buffer, 0, length, StandardCharsets.UTF_8);
        if (process.exitValue() != 0) throw new IllegalStateException(output.trim());
        return output;
    }

    @SuppressWarnings("deprecation")
    private void watch() {
        locations.requestLocationUpdates("gps", 0L, 0f, new LocationListener() {
            @Override public void onLocationChanged(Location fix) {
                try { emit(new JSONObject().put("latitude", fix.getLatitude()).put("longitude", fix.getLongitude())
                    .put("speed", fix.getSpeed()).put("heading", fix.getBearing()).put("altitude", fix.getAltitude())
                    .put("accuracy", fix.getAccuracy()).put("mock", fix.isFromMockProvider())); }
                catch (Exception error) { failure(error); }
            }
            @Override public void onStatusChanged(String provider, int status, Bundle extras) {}
            @Override public void onProviderEnabled(String provider) {}
            @Override public void onProviderDisabled(String provider) {}
        }, Looper.getMainLooper());
        Looper.loop();
    }

    private static synchronized void emit(JSONObject value) { System.out.println(value); System.out.flush(); }
    private static void failure(Throwable error) {
        try { emit(new JSONObject().put("ok", false).put("error", error.toString())); }
        catch (Exception ignored) { System.err.println(error); }
    }
}
