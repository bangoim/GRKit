//! UI copy in English and Simplified Chinese, chosen from the system language.
//!
//! Keys match `locales/app.yml` so `nsh package` and this table stay aligned.

use nsh_core::prelude::*;

fn is_zh() -> bool {
    let tag = locale().to_string();
    tag.starts_with("zh")
}

/// Look up a key; fills `%{name}` style placeholders from `args`.
pub fn tr(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = raw(key).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("%{{{name}}}"), value);
    }
    text
}

fn raw(key: &str) -> &'static str {
    if is_zh() {
        zh(key).unwrap_or_else(|| en(key))
    } else {
        en(key)
    }
}

fn en(key: &str) -> &'static str {
    match key {
        "tab.camera" => "Camera",
        "tab.settings" => "Settings",
        "home.title" => "Add Your Camera",
        "home.subtitle" => "Connect your camera. Manage GRKit.",
        "home.add" => "Add Camera",
        "home.footnote" => "Needs GRKit 1.7.0 or later on the camera.",
        "error.old_title" => "GRKit Needs an Update",
        "error.old_body" => "Install GRKit 1.7.0 or later on the card first.",
        "nearby.powered_on" => "Powered on",
        "nearby.hint" => {
            "First time with this camera? On the camera, select Bluetooth Setting → Action Mode On → Pairing → Execute Pairing"
        }
        "pair.hint_gr3" => {
            "First time with this camera? On the camera, select Bluetooth Setting → Action Mode On → Pairing → Execute Pairing"
        }
        "pair.hint_gr4" => {
            "First time with this camera? On the camera, select Wireless Communication → Action Mode On → Pairing → Execute Pairing"
        }
        "nearby.connect" => "Connect",
        "nearby.not_now" => "Not Now",
        "scan.title" => "Add a Camera",
        "scan.subtitle" => "Keep it close by, with Bluetooth on.",
        "scan.looking" => "Looking for cameras",
        "scan.nearby_off" => "Nearby",
        "scan.hint" => {
            "First time? On the camera: Bluetooth Setting → Action Mode On → Pairing → Execute Pairing"
        }
        "scan.close" => "Close",
        "connecting.cancel" => "Cancel",
        "connecting.title" => "Connecting to %{model}",
        "connecting.keep_on" => "Keep the camera on and nearby.",
        "connecting.bt" => "Connect over Bluetooth",
        "connecting.wifi" => "Join the camera's Wi-Fi",
        "connecting.wifi_sub" => "If iOS asks, allow it.",
        "connecting.version" => "Read GRKit version",
        "connected.status" => "Connected",
        "connected.offline" => "Not Connected",
        "connected.whats_new" => "What's New",
        "connected.update" => "Update",
        "connected.grkit" => "GRKit",
        "connected.firmware" => "Firmware",
        "connected.up_to_date" => "GRKit Is Up to Date",
        "connected.software_update" => "Software Update",
        "software.title" => "Software Update",
        "software.installed" => "Installed",
        "software.looks_stay" => "Your looks and settings stay.",
        "software.roll_back_to" => "Roll Back to %{version}",
        "software.roll_footnote" => "Available until the next update.",
        "software.update_now" => "Update Now",
        "software.confirm_note" => "Confirm on the camera, then restart it.",
        "software.up_to_date" => "GRKit is up to date",
        "software.learn_more" => "Learn More…",
        "sending.title" => "Updating %{model}",
        "sending.progress" => "Sending to your camera…",
        "sending.step_send" => "Send to camera",
        "sending.step_confirm" => "Confirm on camera",
        "sending.step_restart" => "Restart camera",
        "sending.keep_on" => "Keep the camera on.",
        "sending.cancel" => "Cancel",
        "confirm.title" => "Confirm on the Camera",
        "confirm.subtitle" => "The update is on the camera and verified.",
        "confirm.step1" => "MENU → About GRKit",
        "confirm.step2" => "Choose Install",
        "confirm.step3" => "Choose Execute",
        "confirm.waiting" => "Waiting for the camera · %{time}",
        "restart.title" => "Restart the Camera",
        "restart.subtitle" => "Turn it off and on. It installs as it starts.",
        "restart.done" => "Done",
        "restart.cancel" => "Cancel Update",
        "installed.title" => "GRKit %{version} Is Installed",
        "installed.subtitle" => "You can roll back to %{previous} in Software Update.",
        "installed.done" => "Done",
        "roll.alert_title" => "Roll Back to GRKit %{version}?",
        "roll.alert_body" => "Confirm on the camera, then restart it.",
        "roll.confirm" => "Roll Back",
        "roll.cancel" => "Cancel",
        "roll.confirm_title" => "Confirm Roll Back on the Camera",
        "roll.confirm_subtitle" => "Then restart the camera.",
        "error.card_full_title" => "Card Is Full",
        "error.card_full_body" => "Free %{size}, then try again.",
        "error.not_confirmed_title" => "Not Confirmed",
        "error.not_confirmed_body" => "Confirm within 5 minutes. Nothing changed.",
        "error.refused_title" => "Update Refused",
        "error.refused_body" => "The camera refused the update. Nothing changed.",
        "error.lost_title" => "Connection Lost",
        "error.lost_body" => "The link dropped. Nothing changed on the camera.",
        "error.pair_title" => "Pairing Needed",
        "error.pair_body" => "Complete pairing on the camera, then try again.",
        "error.wifi_title" => "Could Not Join Wi-Fi",
        "error.wifi_body" => "Allow the network prompt, keep the camera nearby, then try again.",
        "error.card_ready_title" => "Camera Not Ready",
        "error.card_ready_body" => "The memory card is still starting. Try again in a moment.",
        "error.bt_title" => "Bluetooth Unavailable",
        "error.bt_body" => "Turn on Bluetooth and allow access for GRKit.",
        "error.try_again" => "Try Again",
        "error.send_again" => "Send Again",
        "error.close" => "Close",
        "settings.title" => "Settings",
        "settings.about" => "About",
        "settings.version" => "Version",
        "settings.rate" => "Rate GRKit",
        "settings.privacy" => "Privacy Policy",
        "settings.more_maker" => "More from the Maker",
        "settings.gr_snap_name" => "GR Snap",
        "settings.gr_snap_desc" => "Wireless transfer and remote for RICOH GR",
        "settings.view" => "View",
        _ => "???",
    }
}

fn zh(key: &str) -> Option<&'static str> {
    Some(match key {
        "tab.camera" => "相机",
        "tab.settings" => "设置",
        "home.title" => "添加你的相机",
        "home.subtitle" => "连接相机，管理 GRKit。",
        "home.add" => "添加相机",
        "home.footnote" => "相机上需要 GRKit 1.7.0 或更高版本。",
        "error.old_title" => "需要更新 GRKit",
        "error.old_body" => "请先在卡上安装 GRKit 1.7.0 或更高版本。",
        "nearby.powered_on" => "已开机",
        "nearby.hint" => "首次连接？请在相机上选择“蓝牙设置 → 打开动作模式 → 配对 → 执行配对”",
        "pair.hint_gr3" => "首次连接？请在相机上选择“蓝牙设置 → 打开动作模式 → 配对 → 执行配对”",
        "pair.hint_gr4" => "首次连接？请在相机上选择“无线通信 → 打开动作模式 → 配对 → 执行配对”",
        "nearby.connect" => "连接",
        "nearby.not_now" => "暂不",
        "scan.title" => "添加相机",
        "scan.subtitle" => "请保持相机在附近，并打开蓝牙。",
        "scan.looking" => "正在查找相机",
        "scan.nearby_off" => "在附近",
        "scan.hint" => "首次连接？在相机上：蓝牙设置 → 打开动作模式 → 配对 → 执行配对",
        "scan.close" => "关闭",
        "connecting.cancel" => "取消",
        "connecting.title" => "正在连接 %{model}",
        "connecting.keep_on" => "请保持相机开机并在附近。",
        "connecting.bt" => "通过蓝牙连接",
        "connecting.wifi" => "加入相机的 Wi-Fi",
        "connecting.wifi_sub" => "若系统询问，请允许。",
        "connecting.version" => "读取 GRKit 版本",
        "connected.status" => "已连接",
        "connected.offline" => "未连接",
        "connected.whats_new" => "新功能",
        "connected.update" => "更新",
        "connected.grkit" => "GRKit",
        "connected.firmware" => "固件",
        "connected.up_to_date" => "GRKit 已是最新",
        "connected.software_update" => "软件更新",
        "software.title" => "软件更新",
        "software.installed" => "已安装",
        "software.looks_stay" => "你的效果和设置会保留。",
        "software.roll_back_to" => "回退到 %{version}",
        "software.roll_footnote" => "在下次更新前可用。",
        "software.update_now" => "立即更新",
        "software.confirm_note" => "请在相机上确认，然后重启相机。",
        "software.up_to_date" => "GRKit 已是最新",
        "software.learn_more" => "了解更多…",
        "sending.title" => "正在更新 %{model}",
        "sending.progress" => "正在发送到相机…",
        "sending.step_send" => "发送到相机",
        "sending.step_confirm" => "在相机上确认",
        "sending.step_restart" => "重启相机",
        "sending.keep_on" => "请保持相机开机。",
        "sending.cancel" => "取消",
        "confirm.title" => "在相机上确认",
        "confirm.subtitle" => "更新已在相机上并通过校验。",
        "confirm.step1" => "菜单 → 关于 GRKit",
        "confirm.step2" => "选择安装",
        "confirm.step3" => "选择执行",
        "confirm.waiting" => "等待相机 · %{time}",
        "restart.title" => "重启相机",
        "restart.subtitle" => "关机再开机。启动时会完成安装。",
        "restart.done" => "完成",
        "restart.cancel" => "取消更新",
        "installed.title" => "GRKit %{version} 已安装",
        "installed.subtitle" => "可在“软件更新”中回退到 %{previous}。",
        "installed.done" => "完成",
        "roll.alert_title" => "回退到 GRKit %{version}？",
        "roll.alert_body" => "请在相机上确认，然后重启相机。",
        "roll.confirm" => "回退",
        "roll.cancel" => "取消",
        "roll.confirm_title" => "在相机上确认回退",
        "roll.confirm_subtitle" => "然后重启相机。",
        "error.card_full_title" => "存储卡已满",
        "error.card_full_body" => "请先腾出 %{size}，然后重试。",
        "error.not_confirmed_title" => "未确认",
        "error.not_confirmed_body" => "请在 5 分钟内确认。相机未改动。",
        "error.refused_title" => "更新被拒绝",
        "error.refused_body" => "相机拒绝了此次更新。未改动。",
        "error.lost_title" => "连接已断开",
        "error.lost_body" => "连接中断。相机未改动。",
        "error.pair_title" => "需要配对",
        "error.pair_body" => "请先在相机上完成配对，然后重试。",
        "error.wifi_title" => "无法加入 Wi-Fi",
        "error.wifi_body" => "请允许系统网络提示，保持相机在附近，然后重试。",
        "error.card_ready_title" => "相机未就绪",
        "error.card_ready_body" => "存储卡仍在启动，请稍后再试。",
        "error.bt_title" => "蓝牙不可用",
        "error.bt_body" => "请打开蓝牙并允许 GRKit 使用。",
        "error.try_again" => "重试",
        "error.send_again" => "重新发送",
        "error.close" => "关闭",
        "settings.title" => "设置",
        "settings.about" => "关于",
        "settings.version" => "版本",
        "settings.rate" => "为 GRKit 评分",
        "settings.privacy" => "隐私政策",
        "settings.more_maker" => "来自同一作者",
        "settings.gr_snap_name" => "GR Snap",
        "settings.gr_snap_desc" => "RICOH GR 的无线传输与遥控",
        "settings.view" => "查看",
        _ => return None,
    })
}

/// Format bytes as a short size label for errors (`12 MB`).
pub fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", (bytes + MB / 2) / MB)
    } else if bytes >= 1024 {
        format!("{} KB", (bytes + 512) / 1024)
    } else {
        format!("{bytes} B")
    }
}
