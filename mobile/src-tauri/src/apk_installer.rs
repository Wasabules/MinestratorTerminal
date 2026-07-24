//! Mini-plugin mobile : lance l'installeur système Android sur un APK.
//!
//! Nécessaire car `tauri-plugin-opener` 2.5.4 a un bug (`open_path` envoie une chaîne brute
//! au command Android `open` qui attend un objet → « Cannot construct OpenArgs »), et de toute
//! façon `open` ne fait pas d'intent d'install (pas de FileProvider ni de MIME package-archive).
//! On délègue à une classe Kotlin `ApkInstallerPlugin` via `run_mobile_plugin`.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.geoffreylecoq.minestratorterminal";

pub struct ApkInstaller<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    _marker: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> ApkInstaller<R> {
    #[allow(unused_variables)]
    pub fn install(&self, path: String) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<()>("install", serde_json::json!({ "path": path }))
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            Err("Installation d'APK disponible uniquement sur Android.".into())
        }
    }
}

/// Plugin à enregistrer sur le builder Tauri (`.plugin(apk_installer::init())`).
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("apkinstaller")
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            let handle = _api.register_android_plugin(PLUGIN_IDENTIFIER, "ApkInstallerPlugin")?;
            app.manage(ApkInstaller::<R> {
                #[cfg(target_os = "android")]
                handle,
                #[cfg(not(target_os = "android"))]
                _marker: std::marker::PhantomData,
            });
            Ok(())
        })
        .build()
}

/// Commande exposée au front : lance l'installeur sur l'APK téléchargé.
#[tauri::command]
pub fn install_apk<R: Runtime>(app: tauri::AppHandle<R>, path: String) -> Result<(), String> {
    app.state::<ApkInstaller<R>>().install(path)
}
