mod image_backend;
mod inputs;
mod jobs;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(jobs::JobManager::default())
        .setup(|app| {
            let root = if cfg!(debug_assertions) {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/image-backend")
            } else {
                app.path().resource_dir()?.join("image-backend")
            };
            app.manage(image_backend::ImageBackend { root });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if let Some(job) = window
                    .state::<jobs::JobManager>()
                    .cancel_before_close(window.label())
                {
                    api.prevent_close();
                    let window = window.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        while job.active() {
                            std::thread::sleep(std::time::Duration::from_millis(35));
                        }
                        let _ = window.close();
                    });
                }
            }
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window
                    .state::<jobs::JobManager>()
                    .cancel_window(window.label());
            }
        })
        .invoke_handler(tauri::generate_handler![
            inputs::inspect_inputs,
            jobs::start_conversion,
            jobs::retry_conversion,
            jobs::cancel_conversion,
            jobs::backend_status,
            jobs::reveal_output
        ])
        .run(tauri::generate_context!())
        .expect("Recast could not start");
}
