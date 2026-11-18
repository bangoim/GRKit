//! Connection error copy in English and Simplified Chinese. Keys match `locales/app.yml`.

pub fn en(key: &str) -> Option<&'static str> {
    Some(match key {
        "conn.open_settings" => "Open Settings",
        "conn.ble_failed.title" => "Bluetooth Connection Failed",
        "conn.ble_failed.body" => "Toggle Bluetooth off and on, then retry.",
        "conn.ble_auth.title" => "Pairing Didn't Complete",
        "conn.ble_auth.body" => {
            "The camera didn't finish pairing. On the camera, select Pairing → Execute Pairing, then try again. If that fails, forget this camera in Settings → Bluetooth first."
        }
        "conn.ble_pair.title" => "Pair on the Camera",
        "conn.ble_pair.body" => {
            "On the camera, select Pairing → Execute Pairing, then tap Try Again."
        }
        "conn.ble_stale.title" => "Camera Forgot This Device",
        "conn.ble_stale.body" => {
            "In Settings → Bluetooth, tap ⓘ next to this camera and choose Forget This Device. Then pair again on the camera and try again."
        }
        "conn.ble_timeout.title" => "Camera Not Responding",
        "conn.ble_timeout.body" => "Turn the camera on and keep it close by, then try again.",
        "conn.wifi_denied.title" => "Wi-Fi Join Canceled",
        "conn.wifi_denied.body" => {
            "GRKit needs to join your camera's Wi-Fi network to connect. Tap Join on the next system prompt, or join the camera's Wi-Fi manually from the Wi-Fi section of the Settings app."
        }
        "conn.wifi_password.title" => "Wrong Wi-Fi Password",
        "conn.wifi_password.body" => {
            "The camera's Wi-Fi password may have changed. Pair on the camera again, then try again."
        }
        "conn.wifi_ssid.title" => "Invalid Wi-Fi Name",
        "conn.wifi_ssid.body" => "Check the camera's SSID.",
        "conn.wifi_config.title" => "Wi-Fi Settings Rejected",
        "conn.wifi_config.body" => "iOS couldn't apply these Wi-Fi settings. Try again.",
        "android.conn.wifi_config.body" => {
            "Android couldn't apply these Wi-Fi settings. Try again."
        }
        "conn.wifi_failed.title" => "Camera Wi-Fi Not Ready",
        "conn.wifi_failed.body" => {
            "The camera's Wi-Fi hasn't started yet. Keep the camera on for a few seconds, then try again."
        }
        "conn.wifi_timeout.title" => "Camera Wi-Fi Unreachable",
        "conn.wifi_timeout.body" => "Make sure the camera is on and its Wi-Fi is enabled.",
        "conn.network_not_ready.title" => "Still Switching to Camera Wi-Fi",
        "conn.network_not_ready.body" => {
            "Your device joined the camera's Wi-Fi but hasn't switched over yet. Wait a moment, then try again."
        }
        "conn.subnet.title" => "Wi-Fi Address Conflict",
        "conn.subnet.body" => {
            "Your Wi-Fi hands out the same addresses as the camera (192.168.0.x), so GRKit keeps reaching your router instead. Turn on Airplane Mode and switch only Wi-Fi back on, or pick the camera's Wi-Fi in the Settings app, then return here."
        }
        "conn.verify.title" => "Camera Isn't Responding",
        "conn.verify.body" => "Wi-Fi joined but no reply. Restart the camera.",
        "conn.local_network.title" => "Local Network Access Denied",
        "conn.local_network.body" => {
            "GRKit needs Local Network access to talk to your camera. Enable it in Settings → GRKit → Local Network."
        }
        "android.conn.local_network.body" => {
            "GRKit needs the Nearby devices permission to talk to your camera over Wi-Fi. Tap Open Settings, then allow it in Permissions → Nearby devices."
        }
        "conn.not_gr.title" => "Not a GR Camera",
        "conn.not_gr.body" => {
            "Looks like you're on a different Wi-Fi network. Switch to your camera's Wi-Fi."
        }
        "conn.unsupported.title" => "Unsupported Camera Response",
        "conn.unsupported.body" => {
            "The camera replied, but GRKit couldn't understand the response. Update GRKit on the camera and try again."
        }
        "conn.unknown.title" => "Couldn't Connect",
        "conn.unknown.body" => "Something went wrong. Please retry.",
        _ => return None,
    })
}

pub fn zh(key: &str) -> Option<&'static str> {
    Some(match key {
        "conn.open_settings" => "打开设置",
        "conn.ble_failed.title" => "蓝牙连接失败",
        "conn.ble_failed.body" => "请关闭蓝牙后再打开，然后重试。",
        "conn.ble_auth.title" => "配对未完成",
        "conn.ble_auth.body" => {
            "相机没有完成配对。请在相机上选择「配对」→「执行配对」，然后重试。如果仍然失败，请先在「设置」→「蓝牙」中忽略此相机。"
        }
        "conn.ble_pair.title" => "请在相机上执行蓝牙配对",
        "conn.ble_pair.body" => "请在相机上选择「配对」→「执行配对」，然后点按「重试」。",
        "conn.ble_stale.title" => "相机已不记得此设备",
        "conn.ble_stale.body" => {
            "请在「设置」→「蓝牙」中点击此相机旁的 ⓘ，选择「忽略此设备」。然后在相机上重新配对并重试。"
        }
        "conn.ble_timeout.title" => "相机无响应",
        "conn.ble_timeout.body" => "请打开相机电源并放在附近，然后重试。",
        "conn.wifi_denied.title" => "已取消加入 Wi-Fi",
        "conn.wifi_denied.body" => {
            "GRKit 需要加入相机的 Wi-Fi 才能连接。请在下次系统提示中点击「加入」，或前往「设置」App 的「Wi-Fi」页面手动加入相机 Wi-Fi。"
        }
        "conn.wifi_password.title" => "Wi-Fi 密码错误",
        "conn.wifi_password.body" => "相机的 Wi-Fi 密码可能已更改。请在相机上重新配对，然后重试。",
        "conn.wifi_ssid.title" => "Wi-Fi 名称无效",
        "conn.wifi_ssid.body" => "请检查相机的 SSID。",
        "conn.wifi_config.title" => "Wi-Fi 设置被拒绝",
        "conn.wifi_config.body" => "iOS 无法应用此 Wi-Fi 设置，请重试。",
        "android.conn.wifi_config.body" => "Android 无法应用此 Wi-Fi 设置，请重试。",
        "conn.wifi_failed.title" => "相机 Wi-Fi 尚未就绪",
        "conn.wifi_failed.body" => "相机的 Wi-Fi 还没有启动。请保持相机开机几秒钟，然后重试。",
        "conn.wifi_timeout.title" => "无法连接相机 Wi-Fi",
        "conn.wifi_timeout.body" => "请确认相机已开机且 Wi-Fi 已开启。",
        "conn.network_not_ready.title" => "正在切换到相机 Wi-Fi",
        "conn.network_not_ready.body" => {
            "设备已加入相机 Wi-Fi，但还没有切换过去。请稍等片刻后重试。"
        }
        "conn.subnet.title" => "Wi-Fi 网段与相机冲突",
        "conn.subnet.body" => {
            "你的 Wi-Fi 分配的地址和相机在同一网段（192.168.0.x），GRKit 连上的其实是你的路由器。请先打开飞行模式，再单独打开 Wi-Fi；或者在「设置」App 中手动选择相机的 Wi-Fi，然后回到这里。"
        }
        "conn.verify.title" => "相机无响应",
        "conn.verify.body" => "已加入 Wi-Fi 但相机无响应，请重启相机。",
        "conn.local_network.title" => "本地网络访问被拒绝",
        "conn.local_network.body" => {
            "GRKit 需要本地网络权限才能与相机通信。请前往「设置」→「GRKit」→「本地网络」开启。"
        }
        "android.conn.local_network.body" => {
            "GRKit 需要“附近的设备”权限才能通过 Wi-Fi 与相机通信。请轻点“打开设置”，然后在“权限”→“附近的设备”中允许。"
        }
        "conn.not_gr.title" => "不是 GR 相机",
        "conn.not_gr.body" => "看起来你连接到了其他 Wi-Fi，请切换到相机的 Wi-Fi。",
        "conn.unsupported.title" => "无法识别的相机响应",
        "conn.unsupported.body" => "相机有响应，但 GRKit 无法识别。请更新相机上的 GRKit 后重试。",
        "conn.unknown.title" => "连接失败",
        "conn.unknown.body" => "出了点问题，请重试。",
        _ => return None,
    })
}
