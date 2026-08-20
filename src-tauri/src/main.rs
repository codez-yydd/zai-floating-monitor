// Windows 下所有构建（含 debug）均声明为窗口子系统，
// 避免开机自启指向 debug exe 时弹出控制台黑框，关闭黑框会连带杀死进程。
// tauri dev 从终端启动时标准句柄仍被继承，println 日志不受影响。
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    zai_floating_monitor_lib::run()
}
