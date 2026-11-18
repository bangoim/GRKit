package studio.techflow.grkit;

import android.app.Activity;
import android.content.res.Configuration;
import android.content.pm.ApplicationInfo;
import android.os.Bundle;
import com.longbridge.nsh.GrkitView;

public final class MainActivity extends Activity {
    private GrkitView view;
    private static native void nativeInitialize(String cameraUrl, String releaseBase, String demoScreen, String releasePublicKey);

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        System.loadLibrary("nsh");
        boolean development = (getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0;
        nativeInitialize(BuildConfig.CAMERA_URL, BuildConfig.RELEASE_BASE, BuildConfig.DEMO_SCREEN,
                development ? BuildConfig.RELEASE_PUBLIC_KEY : "");
        CameraBridge.initialize(this);
        view = new GrkitView(this);
        view.registerComponent(ActivityIndicator.NAME, context -> new ActivityIndicator(this));
        setContentView(view);
        view.runRustApp("nsh");
    }

    @Override
    protected void onResume() {
        super.onResume();
        CameraBridge.onResume();
    }

    @Override
    protected void onPause() {
        CameraBridge.onPause();
        super.onPause();
    }

    @Override
    public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        GrkitView.updateSystemBars(this);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        CameraBridge.onRequestPermissionsResult(requestCode, permissions, results);
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        if (view == null || !view.handleBack()) super.onBackPressed();
    }

    @Override
    protected void onDestroy() {
        if (view != null) view.destroy();
        CameraBridge.shutdown();
        super.onDestroy();
    }
}
