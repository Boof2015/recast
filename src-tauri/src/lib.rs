mod aac;
mod animation;
mod audio_backend;
mod audio_extract;
mod avif;
mod desktop;
mod extensions;
mod image_backend;
mod image_format;
mod image_frame;
mod inputs;
mod jobs;
mod mp3;
mod ogg;
mod opus;
mod tiff;
mod video_animation;
mod video_backend;
mod video_frame;
mod video_preview;
mod vorbis;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(jobs::JobManager::default())
        .manage(video_preview::PreviewManager::default())
        .setup(|app| {
            app.manage(extensions::ExtensionManager::new(
                app.path().app_data_dir()?.join("extensions"),
            ));
            let root = if cfg!(debug_assertions) {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/image-backend")
            } else {
                app.path().resource_dir()?.join("image-backend")
            };
            app.manage(image_backend::ImageBackend { root });
            let root = if cfg!(debug_assertions) {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/audio-backend")
            } else {
                app.path().resource_dir()?.join("audio-backend")
            };
            app.manage(audio_backend::AudioBackend { root });
            desktop::install_menu(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let extension =
                    extensions::manager(window.app_handle()).close(Some(window.label()));
                let preview = window
                    .state::<video_preview::PreviewManager>()
                    .close(window.label());
                let job = window
                    .state::<jobs::JobManager>()
                    .cancel_before_close(window.label());
                if extension || job.is_some() || preview.as_ref().is_some_and(|p| p.busy()) {
                    api.prevent_close();
                    let window = window.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        while job.as_ref().is_some_and(|j| j.active())
                            || preview.as_ref().is_some_and(|p| p.busy())
                            || extensions::manager(window.app_handle()).busy(Some(window.label()))
                        {
                            std::thread::sleep(std::time::Duration::from_millis(35));
                        }
                        let _ = window.close();
                    });
                }
            }
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window
                    .state::<video_preview::PreviewManager>()
                    .remove(window.label());
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
            jobs::reveal_output,
            video_preview::preview_video,
            video_preview::preview_video_frame,
            video_preview::preview_animation_frame,
            video_preview::cancel_video_preview,
            extensions::extensions_list,
            extensions::extensions_inspect,
            extensions::extensions_install_local,
            extensions::extensions_catalog,
            extensions::extensions_install,
            extensions::extensions_remove,
            extensions::extensions_cancel
        ])
        .build(tauri::generate_context!())
        .expect("Recast could not start")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                let jobs = app.state::<jobs::JobManager>().cancel_before_exit();
                let previews = app.state::<video_preview::PreviewManager>().close_all();
                let extension = extensions::manager(app).close(None);
                if extension || !jobs.is_empty() || previews.iter().any(|p| p.busy()) {
                    api.prevent_exit();
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        while jobs.iter().any(|job| job.active())
                            || previews.iter().any(|p| p.busy())
                            || extensions::manager(&app).busy(None)
                        {
                            std::thread::sleep(std::time::Duration::from_millis(35));
                        }
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
        });
}
