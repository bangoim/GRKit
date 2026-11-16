package studio.techflow.grkit;

import android.Manifest;
import android.app.Activity;
import android.bluetooth.*;
import android.bluetooth.le.*;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.net.*;
import android.net.wifi.WifiNetworkSpecifier;
import android.os.*;
import android.util.Base64;
import org.json.*;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.*;

/** App-owned radio transport; the shared Rust state machine owns update policy. */
public final class CameraBridge {
    private static final int PERMISSIONS = 4701;
    private static final int WIFI_PERMISSION = 4702;
    private static final UUID INFORMATION = uuid("9A5ED1C5-74CC-4C50-B5B6-66A48E7CCFF1");
    private static final UUID CONTROL = uuid("0F291746-0C80-4726-87A7-3C501FD3B4B6");
    private static final UUID POWER = uuid("B58CE84C-0666-4DE9-BEC8-2D27B27B3211");
    private static final UUID NETWORK = uuid("9111CDD0-9F01-45C4-A2D4-E09E8FB0424D");
    private static final UUID SSID = uuid("90638E5A-E77D-409D-B550-78F7E1CA5AB4");
    private static final UUID PASSWORD = uuid("0F38279C-FE9E-461B-8596-81287E8C9A81");
    private static final UUID CCCD = uuid("00002902-0000-1000-8000-00805f9b34fb");
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final ExecutorService RADIO = Executors.newSingleThreadExecutor();
    private static final ExecutorService HTTP = Executors.newFixedThreadPool(3);
    private static volatile Activity activity;
    private static Context receiverContext;
    private static volatile Session session;
    private static volatile Network cameraNetwork;
    private static volatile CompletableFuture<Network> joining;
    private static volatile CompletableFuture<Boolean> wifiPermission;
    private static final Map<Long, HttpURLConnection> requests = new ConcurrentHashMap<>();
    private static final Set<Long> activeRequests = ConcurrentHashMap.newKeySet();
    private static final Set<Long> cancelledRequests = ConcurrentHashMap.newKeySet();
    private static ConnectivityManager.NetworkCallback networkCallback;
    private static volatile long generation;
    private static boolean scanning;
    private static boolean wantsScan;
    private static boolean requestingPermissions;
    private static final Map<String, Boolean> powered = new ConcurrentHashMap<>();

    private CameraBridge() {}
    private static UUID uuid(String value) { return UUID.fromString(value); }
    private static native void complete(long id, String json, String error);
    private static native void event(String kind, String json);

    private static final BroadcastReceiver ADAPTER_STATE = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) {
            if (!BluetoothAdapter.ACTION_STATE_CHANGED.equals(intent.getAction())) return;
            int state = intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR);
            if (state == BluetoothAdapter.STATE_ON) {
                resumeScan();
            } else if (state == BluetoothAdapter.STATE_OFF || state == BluetoothAdapter.STATE_TURNING_OFF) {
                stopScan();
                event("unavailable", "true");
            }
        }
    };

    public static void initialize(Activity host) {
        activity = host;
        if (receiverContext == null) {
            Context context = host.getApplicationContext();
            IntentFilter filter = new IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED);
            // Bluetooth broadcasts originate in a privileged process outside the app.
            // ACTION_STATE_CHANGED is a protected broadcast, not an app-supplied event.
            if (Build.VERSION.SDK_INT >= 33) {
                context.registerReceiver(ADAPTER_STATE, filter, Context.RECEIVER_EXPORTED);
            } else {
                context.registerReceiver(ADAPTER_STATE, filter);
            }
            receiverContext = context;
        }
    }

    public static void onResume() {
        resumeScan();
    }

    private static void resumeScan() {
        // Returning from Settings may grant access. A denied permission must not
        // prompt again merely because the activity regained focus.
        if (!wantsScan || !radioPermissions()) return;
        Session current = session;
        // BLE setup intentionally pauses scanning while connecting/pairing.
        if (current != null && !current.closed && !current.disconnected) return;
        try { startScan(); }
        catch (SecurityException ignored) { event("unavailable", "true"); }
    }

    public static String remembered() {
        Activity host = activity;
        return host == null ? "null" : host.getSharedPreferences("camera", Context.MODE_PRIVATE)
                .getString("remembered", "null");
    }

    public static void saveRemembered(String json) {
        Activity host = activity;
        if (host == null || !host.getSharedPreferences("camera", Context.MODE_PRIVATE)
                .edit().putString("remembered", json).commit()) {
            throw new IllegalStateException("Camera preferences could not be saved");
        }
    }

    public static void execute(long id, String operation, String json) {
        if (operation.equals("cancelRequest")) {
            try {
                long original = new JSONObject(json).getLong("id");
                synchronized (activeRequests) {
                    if (activeRequests.contains(original)) cancelledRequests.add(original);
                }
                HttpURLConnection request = requests.remove(original);
                if (request != null) request.disconnect();
                complete(id, "null", null);
            } catch (JSONException error) { complete(id, null, "ConnectionLost"); }
            return;
        }
        if (operation.equals("scan") || operation.equals("stopScan") || operation.equals("cancel")) {
            MAIN.post(() -> {
                try {
                    if (operation.equals("scan")) { wantsScan = true; startScan(); }
                    else if (operation.equals("stopScan")) { wantsScan = false; stopScan(); }
                    else cancel();
                    complete(id, "null", null);
                } catch (Exception error) { complete(id, null, errorName(error)); }
            });
            return;
        }
        long attempt = generation;
        if (operation.equals("http")) synchronized (activeRequests) { activeRequests.add(id); }
        (operation.equals("http") ? HTTP : RADIO).execute(() -> {
            try {
                JSONObject args = new JSONObject(json);
                Object result;
                switch (operation) {
                    case "setup": result = setup(args, attempt); break;
                    case "join": join(args, attempt); result = JSONObject.NULL; break;
                    case "http": result = http(id, args); break;
                    default: throw new Failure("ConnectionLost");
                }
                complete(id, result.toString(), null);
            } catch (Exception error) { complete(id, null, errorName(error)); }
            finally {
                synchronized (activeRequests) { activeRequests.remove(id); cancelledRequests.remove(id); }
            }
        });
    }

    private static String errorName(Exception error) {
        Throwable cause = error;
        while (cause instanceof ExecutionException && cause.getCause() != null) cause = cause.getCause();
        if (cause instanceof Failure) return cause.getMessage();
        if (cause instanceof SecurityException) return "BluetoothUnavailable";
        return "ConnectionLost";
    }

    private static BluetoothAdapter adapter() {
        Activity host = activity;
        BluetoothManager manager = host == null ? null : host.getSystemService(BluetoothManager.class);
        return manager == null ? null : manager.getAdapter();
    }

    private static boolean granted(String permission) {
        return activity != null && activity.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED;
    }

    private static boolean radioPermissions() {
        return Build.VERSION.SDK_INT >= 31
                ? granted(Manifest.permission.BLUETOOTH_SCAN) && granted(Manifest.permission.BLUETOOTH_CONNECT)
                : granted(Manifest.permission.ACCESS_FINE_LOCATION);
    }

    private static void startScan() {
        if (!radioPermissions()) {
            if (!requestingPermissions && activity != null) {
                requestingPermissions = true;
                List<String> required = new ArrayList<>();
                if (Build.VERSION.SDK_INT >= 31) {
                    required.add(Manifest.permission.BLUETOOTH_SCAN);
                    required.add(Manifest.permission.BLUETOOTH_CONNECT);
                }
                if (Build.VERSION.SDK_INT <= 32) required.add(Manifest.permission.ACCESS_FINE_LOCATION);
                if (Build.VERSION.SDK_INT >= 33) required.add(Manifest.permission.NEARBY_WIFI_DEVICES);
                activity.requestPermissions(required.toArray(new String[0]), PERMISSIONS);
            }
            return;
        }
        BluetoothAdapter adapter = adapter();
        boolean unavailable = adapter == null || !adapter.isEnabled();
        event("unavailable", Boolean.toString(unavailable));
        if (unavailable || scanning || !wantsScan) return;
        BluetoothLeScanner scanner = adapter.getBluetoothLeScanner();
        if (scanner == null) return;
        scanner.startScan(Arrays.asList(
                new ScanFilter.Builder().setServiceUuid(new ParcelUuid(INFORMATION)).build(),
                new ScanFilter.Builder().setServiceUuid(new ParcelUuid(CONTROL)).build()),
                new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(), SCAN);
        scanning = true;
    }

    public static void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        if (code == WIFI_PERMISSION) {
            CompletableFuture<Boolean> waiting = wifiPermission;
            if (waiting != null) waiting.complete(results.length > 0 && results[0] == PackageManager.PERMISSION_GRANTED);
            return;
        }
        if (code != PERMISSIONS) return;
        requestingPermissions = false;
        if (radioPermissions()) startScan();
        else event("unavailable", "true");
    }

    private static void stopScan() {
        if (!scanning) return;
        scanning = false;
        try {
            BluetoothAdapter adapter = adapter();
            if (adapter != null && adapter.getBluetoothLeScanner() != null)
                adapter.getBluetoothLeScanner().stopScan(SCAN);
        } catch (SecurityException ignored) { /* Permission can be revoked in Settings. */ }
    }

    private static final ScanCallback SCAN = new ScanCallback() {
        @Override public void onScanResult(int type, ScanResult result) {
            ScanRecord record = result.getScanRecord();
            if (record == null) return;
            try {
                byte[] data = record.getManufacturerSpecificData(0x065f);
                JSONArray manufacturer = new JSONArray();
                if (data != null) {
                    manufacturer.put(0x5f).put(0x06);
                    for (byte value : data) manufacturer.put(value & 255);
                }
                JSONArray services = new JSONArray();
                boolean information = false;
                if (record.getServiceUuids() != null) for (ParcelUuid service : record.getServiceUuids()) {
                    services.put(service.toString());
                    information |= service.getUuid().equals(INFORMATION);
                }
                boolean on = false;
                if (data != null && data.length > 0 && data[0] == (byte) 0xda) {
                    for (int offset = 1; offset < data.length;) {
                        int field = data[offset] & 255;
                        int length = field == 2 ? 4 : (field == 1 || field == 3 ? 1 : 0);
                        if (length == 0 || offset + length >= data.length) break;
                        if (field == 3) on = data[offset + 1] == 1;
                        offset += length + 1;
                    }
                }
                String id = result.getDevice().getAddress();
                powered.put(id, on && information);
                event("scan", new JSONObject().put("id", id).put("name", record.getDeviceName())
                        .put("manufacturer", manufacturer).put("services", services)
                        .put("rssi", result.getRssi()).toString());
            } catch (JSONException | SecurityException ignored) { }
        }
        @Override public void onScanFailed(int error) {
            scanning = false;
            event("unavailable", "true");
        }
    };

    private static JSONObject setup(JSONObject args, long attempt) throws Exception {
        if (!radioPermissions()) throw new Failure("BluetoothUnavailable");
        BluetoothAdapter adapter = adapter();
        if (adapter == null || !adapter.isEnabled()) throw new Failure("BluetoothUnavailable");
        MAIN.post(CameraBridge::stopScan);
        pause(500, attempt);
        String id = args.getString("id");
        String model = args.getString("model");
        BluetoothDevice device;
        try { device = adapter.getRemoteDevice(id); }
        catch (IllegalArgumentException error) { throw new Failure("NotNearby"); }
        Session link = null;
        for (int retry = 0; retry < 3; retry++) {
            checkAttempt(attempt);
            if (session != null) session.close();
            link = new Session(device, attempt);
            session = link;
            try { link.connect(); break; }
            catch (Exception error) {
                link.close();
                if (retry == 2) throw error;
                pause(2000, attempt);
            }
        }
        try {
            link.discover();
            String ssid = credential(link.read(SSID));
            String password = credential(link.read(PASSWORD));
            if (ssid.isEmpty() || password.isEmpty()) throw new Failure("NotPaired");
            boolean alreadyOn = Boolean.TRUE.equals(powered.get(id));
            if (!alreadyOn) {
                try { link.write(POWER, new byte[]{1}); }
                catch (Failure ignored) { /* Some firmware wakes before acknowledging. */ }
            }
            boolean ready = false;
            boolean subscribed = false;
            try { link.subscribe(); subscribed = true; }
            catch (Exception ignored) { /* Starting the AP does not require notification support. */ }
            try {
                for (int retry = 0; retry < 2 && !ready; retry++) {
                    link.write(NETWORK, new byte[]{1});
                    ready = subscribed && link.apReady.poll(2, TimeUnit.SECONDS) != null;
                    if (!ready && retry == 0) pause(500, attempt);
                }
            } catch (Exception ignored) {
                // AP startup can drop GATT. Credentials remain valid; HTTP verifies readiness.
            }
            pause((model.equals("gr4") ? 8000 : 1500) + (ready ? 0 : 1500), attempt);
            checkAttempt(attempt);
            return new JSONObject().put("peripheral_id", id).put("model", model)
                    .put("powered_on_already", alreadyOn).put("credentials", new JSONObject()
                            .put("ssid", ssid).put("passphrase", password));
        } catch (Exception error) {
            link.close();
            MAIN.post(() -> { if (wantsScan) startScan(); });
            throw error;
        }
    }

    private static String credential(byte[] bytes) {
        int end = 0;
        while (end < bytes.length && bytes[end] != 0) end++;
        return new String(bytes, 0, end, StandardCharsets.UTF_8).trim();
    }

    private static void checkAttempt(long attempt) throws Failure {
        if (attempt != generation || activity == null) throw new Failure("Cancelled");
    }

    private static void pause(long milliseconds, long attempt) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + milliseconds;
        while (SystemClock.elapsedRealtime() < deadline) {
            checkAttempt(attempt);
            Thread.sleep(Math.min(250, Math.max(1, deadline - SystemClock.elapsedRealtime())));
        }
        checkAttempt(attempt);
    }

    private static final class Failure extends Exception {
        private static final long serialVersionUID = 1L;
        Failure(String message) { super(message); }
    }

    private static final class Result {
        final int status;
        final byte[] data;
        Result(int status, byte[] data) { this.status = status; this.data = data; }
    }

    /** One outstanding GATT operation; callbacks never block the Android binder thread. */
    private static final class Session extends BluetoothGattCallback {
        final BluetoothDevice device;
        final long attempt;
        final BlockingQueue<Boolean> apReady = new LinkedBlockingQueue<>();
        volatile BluetoothGatt gatt;
        volatile CompletableFuture<Result> pending;
        volatile UUID expected;
        volatile String operation;
        volatile boolean closed;
        volatile boolean disconnected;
        Session(BluetoothDevice device, long attempt) { this.device = device; this.attempt = attempt; }
        void begin(String kind, UUID uuid) throws Failure {
            checkAttempt(attempt);
            if (closed) throw new Failure("ConnectionLost");
            operation = kind; expected = uuid; pending = new CompletableFuture<>();
        }
        Result await() throws Exception {
            try { return pending.get(15, TimeUnit.SECONDS); }
            catch (TimeoutException error) {
                // Android may hold the encrypted read while its pairing dialog is open.
                if (operation.equals("read") && device.getBondState() == BluetoothDevice.BOND_BONDING) {
                    try { return pending.get(60, TimeUnit.SECONDS); }
                    catch (TimeoutException pairingTimeout) { close(); throw new Failure("PairingFailed"); }
                }
                close(); throw new Failure("ConnectionLost");
            }
        }
        void deliver(String kind, UUID uuid, int status, byte[] bytes) {
            CompletableFuture<Result> waiting = pending;
            if (!closed && waiting != null && kind.equals(operation) && Objects.equals(uuid, expected))
                waiting.complete(new Result(status, bytes));
        }
        void success(Result result) throws Failure {
            if (result.status != BluetoothGatt.GATT_SUCCESS) throw new Failure("ConnectionLost");
        }
        void connect() throws Exception {
            begin("connect", null);
            gatt = device.connectGatt(activity, false, this, BluetoothDevice.TRANSPORT_LE);
            if (gatt == null) throw new Failure("ConnectionLost");
            success(await());
        }
        void discover() throws Exception {
            begin("discover", null);
            if (!gatt.discoverServices()) throw new Failure("ConnectionLost");
            success(await());
        }
        BluetoothGattCharacteristic characteristic(UUID uuid) throws Failure {
            if (gatt != null) for (BluetoothGattService service : gatt.getServices()) {
                BluetoothGattCharacteristic value = service.getCharacteristic(uuid);
                if (value != null) return value;
            }
            throw new Failure("NotPaired");
        }
        byte[] read(UUID uuid) throws Exception {
            BluetoothGattCharacteristic value = characteristic(uuid);
            for (int retry = 0; retry < 2; retry++) {
                begin("read", uuid);
                if (!gatt.readCharacteristic(value)) throw new Failure("ConnectionLost");
                Result result = await();
                if (result.status == BluetoothGatt.GATT_SUCCESS) return result.data;
                if (result.status != BluetoothGatt.GATT_INSUFFICIENT_AUTHENTICATION
                        && result.status != BluetoothGatt.GATT_INSUFFICIENT_ENCRYPTION
                        && result.status != 137) throw new Failure("ConnectionLost");
                if (retry != 0) throw new Failure("NotPaired");
                if (device.getBondState() == BluetoothDevice.BOND_NONE && !device.createBond())
                    throw new Failure("PairingFailed");
                long deadline = SystemClock.elapsedRealtime() + 60000;
                long bondingGrace = SystemClock.elapsedRealtime() + 2000;
                do {
                    checkAttempt(attempt);
                    Thread.sleep(250);
                    if (device.getBondState() == BluetoothDevice.BOND_BONDED) break;
                    if (device.getBondState() == BluetoothDevice.BOND_NONE
                            && SystemClock.elapsedRealtime() >= bondingGrace) throw new Failure("PairingFailed");
                } while (SystemClock.elapsedRealtime() < deadline);
                if (device.getBondState() != BluetoothDevice.BOND_BONDED) throw new Failure("PairingFailed");
            }
            throw new Failure("NotPaired");
        }
        @SuppressWarnings("deprecation")
        void write(UUID uuid, byte[] bytes) throws Exception {
            BluetoothGattCharacteristic value = characteristic(uuid);
            begin("write", uuid);
            boolean accepted;
            if (Build.VERSION.SDK_INT >= 33) accepted = gatt.writeCharacteristic(value, bytes,
                    BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT) == BluetoothStatusCodes.SUCCESS;
            else {
                value.setWriteType(BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT);
                value.setValue(bytes);
                accepted = gatt.writeCharacteristic(value);
            }
            if (!accepted) throw new Failure("ConnectionLost");
            success(await());
        }
        @SuppressWarnings("deprecation")
        void subscribe() throws Exception {
            BluetoothGattCharacteristic value = characteristic(NETWORK);
            if (!gatt.setCharacteristicNotification(value, true)) throw new Failure("ConnectionLost");
            BluetoothGattDescriptor descriptor = value.getDescriptor(CCCD);
            if (descriptor == null) throw new Failure("ConnectionLost");
            begin("descriptor", CCCD);
            byte[] bytes = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE;
            boolean accepted;
            if (Build.VERSION.SDK_INT >= 33) accepted = gatt.writeDescriptor(descriptor, bytes) == BluetoothStatusCodes.SUCCESS;
            else { descriptor.setValue(bytes); accepted = gatt.writeDescriptor(descriptor); }
            if (!accepted) throw new Failure("ConnectionLost");
            success(await());
        }
        void close() {
            closed = true;
            CompletableFuture<Result> waiting = pending;
            if (waiting != null) waiting.completeExceptionally(new Failure("ConnectionLost"));
            BluetoothGatt old = gatt;
            gatt = null;
            if (old != null) {
                try { old.disconnect(); old.close(); } catch (SecurityException ignored) { }
            }
        }
        @Override public void onConnectionStateChange(BluetoothGatt source, int status, int state) {
            if (closed) return;
            if (state == BluetoothProfile.STATE_CONNECTED) {
                disconnected = false;
                deliver("connect", null, status, null);
            } else if (state == BluetoothProfile.STATE_DISCONNECTED) {
                disconnected = true;
                CompletableFuture<Result> waiting = pending;
                if (waiting != null) waiting.completeExceptionally(new Failure("ConnectionLost"));
                event("disconnected", JSONObject.quote(device.getAddress()));
            }
        }
        @Override public void onServicesDiscovered(BluetoothGatt source, int status) { deliver("discover", null, status, null); }
        @Override public void onCharacteristicRead(BluetoothGatt source, BluetoothGattCharacteristic value, byte[] bytes, int status) {
            deliver("read", value.getUuid(), status, bytes.clone());
        }
        @Override @SuppressWarnings("deprecation")
        public void onCharacteristicRead(BluetoothGatt source, BluetoothGattCharacteristic value, int status) {
            if (Build.VERSION.SDK_INT < 33) {
                byte[] bytes = value.getValue();
                deliver("read", value.getUuid(), status, bytes == null ? new byte[0] : bytes.clone());
            }
        }
        @Override public void onCharacteristicWrite(BluetoothGatt source, BluetoothGattCharacteristic value, int status) {
            deliver("write", value.getUuid(), status, null);
        }
        @Override public void onDescriptorWrite(BluetoothGatt source, BluetoothGattDescriptor value, int status) {
            deliver("descriptor", value.getUuid(), status, null);
        }
        @Override public void onCharacteristicChanged(BluetoothGatt source, BluetoothGattCharacteristic value, byte[] bytes) {
            if (!closed && value.getUuid().equals(NETWORK) && bytes != null && bytes.length > 0 && bytes[0] == 1) apReady.offer(true);
        }
        @Override @SuppressWarnings("deprecation")
        public void onCharacteristicChanged(BluetoothGatt source, BluetoothGattCharacteristic value) {
            if (Build.VERSION.SDK_INT < 33) onCharacteristicChanged(source, value, value.getValue());
        }
    }

    private static void join(JSONObject args, long attempt) throws Exception {
        if (Build.VERSION.SDK_INT < 29) throw new Failure("WifiJoinFailed");
        String permission = Build.VERSION.SDK_INT >= 33 ? Manifest.permission.NEARBY_WIFI_DEVICES
                : Manifest.permission.ACCESS_FINE_LOCATION;
        if (!granted(permission)) {
            CompletableFuture<Boolean> allowed = new CompletableFuture<>();
            wifiPermission = allowed;
            MAIN.post(() -> {
                Activity host = activity;
                if (host != null && attempt == generation) host.requestPermissions(new String[]{permission}, WIFI_PERMISSION);
                else allowed.complete(false);
            });
            try {
                if (!allowed.get(90, TimeUnit.SECONDS)) throw new Failure("WifiJoinRejected");
            } finally { if (wifiPermission == allowed) wifiPermission = null; }
        }
        checkAttempt(attempt);
        String ssid = args.getString("ssid");
        CompletableFuture<Network> ready = new CompletableFuture<>();
        joining = ready;
        ConnectivityManager manager = activity.getSystemService(ConnectivityManager.class);
        WifiNetworkSpecifier specifier = new WifiNetworkSpecifier.Builder().setSsid(ssid)
                .setWpa2Passphrase(args.getString("passphrase")).build();
        NetworkRequest request = new NetworkRequest.Builder().addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET).setNetworkSpecifier(specifier).build();
        ConnectivityManager.NetworkCallback callback = new ConnectivityManager.NetworkCallback() {
            @Override public void onAvailable(Network network) {
                synchronized (CameraBridge.class) {
                    if (attempt == generation && networkCallback == this) {
                        cameraNetwork = network; ready.complete(network);
                    }
                }
            }
            @Override public void onUnavailable() { ready.completeExceptionally(new Failure("WifiJoinRejected")); }
            @Override public void onLost(Network network) {
                synchronized (CameraBridge.class) {
                    if (networkCallback == this && network.equals(cameraNetwork)) cameraNetwork = null;
                }
            }
        };
        synchronized (CameraBridge.class) {
            releaseNetwork();
            networkCallback = callback;
            manager.requestNetwork(request, callback, 60000);
        }
        try { ready.get(65, TimeUnit.SECONDS); checkAttempt(attempt); }
        catch (Exception error) {
            synchronized (CameraBridge.class) {
                if (networkCallback == callback) releaseNetwork();
            }
            if (error instanceof TimeoutException) throw new Failure("WifiJoinFailed");
            throw error;
        } finally { if (joining == ready) joining = null; }
    }

    private static synchronized void releaseNetwork() {
        ConnectivityManager.NetworkCallback callback = networkCallback;
        networkCallback = null;
        cameraNetwork = null;
        if (callback != null && activity != null) {
            try { activity.getSystemService(ConnectivityManager.class).unregisterNetworkCallback(callback); }
            catch (IllegalArgumentException ignored) { }
        }
    }

    private static JSONObject http(long id, JSONObject args) throws Exception {
        URL url = new URL(args.getString("url"));
        if (!url.getProtocol().equals("http") && !url.getProtocol().equals("https")) throw new IOException("Unsupported protocol");
        Network network = cameraNetwork;
        HttpURLConnection connection = (HttpURLConnection) (network == null ? url.openConnection() : network.openConnection(url));
        requests.put(id, connection);
        try {
            if (cancelledRequests.contains(id)) throw new Failure("Cancelled");
            int timeout = Math.max(1, Math.min(180000, args.optInt("timeout_ms", 30000)));
            connection.setConnectTimeout(Math.min(3000, timeout));
            connection.setReadTimeout(timeout);
            connection.setInstanceFollowRedirects(false);
            String method = args.getString("method");
            connection.setRequestMethod(method);
            JSONObject headers = args.optJSONObject("headers");
            if (headers != null) for (Iterator<String> keys = headers.keys(); keys.hasNext();) {
                String key = keys.next(); connection.setRequestProperty(key, headers.getString(key));
            }
            if (args.has("body") && !args.isNull("body")
                    && (!args.getString("body").isEmpty() || method.equals("PUT") || method.equals("POST"))) {
                byte[] body = Base64.decode(args.getString("body"), Base64.DEFAULT);
                connection.setDoOutput(true);
                connection.setFixedLengthStreamingMode(body.length);
                try (OutputStream output = connection.getOutputStream()) { output.write(body); }
            }
            int status = connection.getResponseCode();
            JSONObject responseHeaders = new JSONObject();
            for (Map.Entry<String, List<String>> header : connection.getHeaderFields().entrySet())
                if (header.getKey() != null) responseHeaders.put(header.getKey(), String.join(", ", header.getValue()));
            InputStream input = status >= 400 ? connection.getErrorStream() : connection.getInputStream();
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            if (input != null) try (InputStream stream = input) {
                byte[] buffer = new byte[16384];
                for (int count; (count = stream.read(buffer)) != -1;) {
                    if (bytes.size() + count > 16 * 1024 * 1024) throw new IOException("Camera response too large");
                    bytes.write(buffer, 0, count);
                }
            }
            return new JSONObject().put("status", status).put("headers", responseHeaders)
                    .put("body", Base64.encodeToString(bytes.toByteArray(), Base64.NO_WRAP));
        } finally { requests.remove(id); connection.disconnect(); }
    }

    private static void cancel() {
        generation++;
        Session old = session;
        session = null;
        if (old != null) old.close();
        CompletableFuture<Network> pendingJoin = joining;
        if (pendingJoin != null) pendingJoin.completeExceptionally(new Failure("Cancelled"));
        CompletableFuture<Boolean> pendingPermission = wifiPermission;
        if (pendingPermission != null) pendingPermission.completeExceptionally(new Failure("Cancelled"));
        releaseNetwork();
        if (wantsScan) startScan();
    }

    public static void shutdown() {
        if (receiverContext != null) {
            receiverContext.unregisterReceiver(ADAPTER_STATE);
            receiverContext = null;
        }
        wantsScan = false;
        requestingPermissions = false;
        stopScan();
        cancel();
        synchronized (activeRequests) { cancelledRequests.addAll(activeRequests); }
        for (HttpURLConnection request : requests.values()) request.disconnect();
        activity = null;
    }
}
