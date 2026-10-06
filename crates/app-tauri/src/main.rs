// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK can render the entire window black on some Linux GPU/session
    // combinations (notably NVIDIA + Wayland and hybrid-GPU laptops): the page
    // loads but accelerated composition through the DMA-BUF renderer fails.
    // Disable that renderer unless the user has already chosen a value.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    maggcs_app_lib::run()
}
