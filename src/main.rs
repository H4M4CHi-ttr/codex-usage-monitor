#![windows_subsystem = "windows"]

mod diagnose;
mod localization;
mod models;
mod native_interop;
mod poller;
mod provider_icons;
mod provider_poll;
mod quota_refresh;
mod quota_text;
mod quota_tooltip;
mod recovery_events;
mod retry_after;
mod settings_store;
mod theme;
mod tray_icon;
mod updater;
mod window;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    if diagnose_enabled {
        match diagnose::init() {
            Ok(()) => {
                diagnose::install_panic_hook();
                diagnose::log(format!(
                    "startup diagnostic_build=local-instrumentation version={} install_channel={:?} pid={}",
                    env!("CARGO_PKG_VERSION"),
                    updater::current_install_channel(),
                    std::process::id()
                ));
            }
            Err(error) => {
                // Logging may not be available yet, but keep startup behavior unchanged.
                let _ = error;
            }
        }
    }

    if let Some(exit_code) = updater::handle_cli_mode(&args) {
        if diagnose_enabled {
            diagnose::log(format!("cli mode exited with code {exit_code}"));
        }
        std::process::exit(exit_code);
    }

    if diagnose_enabled {
        diagnose::log("entering window::run");
    }
    window::run();
    diagnose::log("window::run returned; main is exiting");
}
