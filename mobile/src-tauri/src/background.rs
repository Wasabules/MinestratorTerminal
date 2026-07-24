//! Surveillance en arrière-plan (alertes app fermée).
//!
//! Démarre/arrête un **service Android au premier plan** ([`MonitorService`] côté Kotlin) dont le
//! seul rôle est de **garder le process de l'app vivant**. Le superviseur Rust (démarré dans
//! `setup()`) et `forward()` continuent alors de poster les alertes même app fermée — on ne
//! duplique pas la surveillance ici.
//!
//! iOS : pas d'équivalent (l'OS interdit les tâches de fond persistantes) → la commande est un
//! no-op. La voie « garantie » multi-plateforme reste le daemon + push (docs/PUSH.md).

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.geoffreylecoq.minestratorterminal";

/// Réponse du plugin Android pour les commandes booléennes (`{ value: bool }`).
#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct BoolValue {
    value: bool,
}

pub struct BackgroundMonitor<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    _marker: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> BackgroundMonitor<R> {
    #[allow(unused_variables)]
    pub fn set(&self, enabled: bool) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            let cmd = if enabled { "start" } else { "stop" };
            self.handle
                .run_mobile_plugin::<()>(cmd, ())
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            // iOS / desktop : pas de service de fond persistant. On ignore silencieusement.
            Ok(())
        }
    }

    /// `true` si l'app est exemptée d'optimisation batterie (peut tourner en arrière-plan).
    pub fn is_battery_unrestricted(&self) -> Result<bool, String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<BoolValue>("isBatteryUnrestricted", ())
                .map(|r| r.value)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            // Rien à restreindre hors Android → considéré « autorisé ».
            Ok(true)
        }
    }

    /// Ouvre la boîte de dialogue système d'exemption d'optimisation batterie.
    #[allow(unused)]
    pub fn request_battery_unrestricted(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<()>("requestBatteryUnrestricted", ())
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            Ok(())
        }
    }

    /// Ouvre la fiche « infos de l'app » (batterie, autostart selon l'OEM, notifications).
    #[allow(unused)]
    pub fn open_app_settings(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<()>("openAppSettings", ())
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            Ok(())
        }
    }
}

/// Plugin à enregistrer sur le builder Tauri (`.plugin(background::init())`).
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("backgroundmonitor")
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            let handle = _api.register_android_plugin(PLUGIN_IDENTIFIER, "BackgroundMonitorPlugin")?;
            app.manage(BackgroundMonitor::<R> {
                #[cfg(target_os = "android")]
                handle,
                #[cfg(not(target_os = "android"))]
                _marker: std::marker::PhantomData,
            });
            Ok(())
        })
        .build()
}

/// Active/désactive la surveillance en arrière-plan (service au premier plan Android).
#[tauri::command]
pub fn set_background_monitoring<R: Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    app.state::<BackgroundMonitor<R>>().set(enabled)
}

/// `true` si l'app est exemptée d'optimisation batterie (fiabilité en arrière-plan).
#[tauri::command]
pub fn is_battery_unrestricted<R: Runtime>(app: tauri::AppHandle<R>) -> Result<bool, String> {
    app.state::<BackgroundMonitor<R>>().is_battery_unrestricted()
}

/// Demande l'exemption d'optimisation batterie (boîte de dialogue système).
#[tauri::command]
pub fn request_battery_unrestricted<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    app.state::<BackgroundMonitor<R>>()
        .request_battery_unrestricted()
}

/// Ouvre la fiche « infos de l'app » (batterie, autostart, notifications).
#[tauri::command]
pub fn open_app_settings<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    app.state::<BackgroundMonitor<R>>().open_app_settings()
}
