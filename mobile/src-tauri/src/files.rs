//! Résolution du nom réel d'un fichier choisi via le sélecteur système.
//!
//! Le sélecteur Android (SAF) renvoie un `content://` opaque dont le dernier segment est un
//! identifiant interne (`…/document/image%3A3604`), ni le nom ni l'extension du fichier. Seul le
//! `ContentResolver` connaît le nom d'affichage : d'où ce petit pont vers [`FilesPlugin`] côté
//! Kotlin. Sans lui, un fichier envoyé arrive sur le serveur nommé « image%3A3604 ».
//!
//! Hors Android, un chemin classique suffit : on renvoie son dernier segment.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.geoffreylecoq.minestratorterminal";

/// Réponse du plugin Android (`{ value: "IMG_2026.jpg" }`).
#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct StringValue {
    value: String,
}

/// Descripteur de fichier cédé par Kotlin, avec la taille annoncée (`-1` si inconnue).
#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
pub struct OpenedFile {
    pub fd: i32,
    pub size: i64,
}

pub struct Files<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    // `fn() -> R` plutôt que `R` : toujours Send + Sync, quel que soit le runtime.
    _marker: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> Files<R> {
    /// Nom d'affichage du fichier désigné par `uri`.
    pub fn display_name(&self, uri: &str) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            #[derive(serde::Serialize)]
            struct Args<'a> {
                uri: &'a str,
            }
            let res: StringValue = self
                .handle
                .run_mobile_plugin("displayName", Args { uri })
                .map_err(|e| e.to_string())?;
            Ok(res.value)
        }
        #[cfg(not(target_os = "android"))]
        {
            // iOS / desktop : `uri` est un vrai chemin, le nom en est le dernier segment.
            Ok(uri
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(uri)
                .to_string())
        }
    }

    /// Ouvre un fichier en lecture et récupère son descripteur, cédé par Kotlin.
    ///
    /// Le descripteur devient la propriété de l'appelant : c'est lui qui le referme (ici, via le
    /// `File` construit par-dessus). Sans cela, il fuirait à chaque envoi.
    #[cfg(target_os = "android")]
    pub fn open_for_read(&self, uri: &str) -> Result<OpenedFile, String> {
        #[derive(serde::Serialize)]
        struct Args<'a> {
            uri: &'a str,
        }
        self.handle
            .run_mobile_plugin::<OpenedFile>("openForRead", Args { uri })
            .map_err(|e| e.to_string())
    }

    /// Affiche ou met à jour la notification d'un transfert. No-op hors Android.
    #[allow(clippy::too_many_arguments)]
    pub fn notify_transfer(
        &self,
        _id: i32,
        _title: &str,
        _text: &str,
        _done: u64,
        _total: u64,
        _finished: bool,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Args<'a> {
                id: i32,
                title: &'a str,
                text: &'a str,
                done: u64,
                total: u64,
                finished: bool,
            }
            self.handle
                .run_mobile_plugin::<()>(
                    "notifyTransfer",
                    Args { id: _id, title: _title, text: _text, done: _done, total: _total, finished: _finished },
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Retire la notification d'un transfert.
    pub fn clear_transfer(&self, _id: i32) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            #[derive(serde::Serialize)]
            struct Args {
                id: i32,
            }
            self.handle
                .run_mobile_plugin::<()>("clearTransfer", Args { id: _id })
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Plugin à enregistrer sur le builder Tauri (`.plugin(files::init())`).
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("files")
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            let handle = _api.register_android_plugin(PLUGIN_IDENTIFIER, "FilesPlugin")?;
            app.manage(Files::<R> {
                #[cfg(target_os = "android")]
                handle,
                #[cfg(not(target_os = "android"))]
                _marker: std::marker::PhantomData,
            });
            Ok(())
        })
        .build()
}

/// Nom réel d'un fichier choisi dans le sélecteur, à partir de son URI ou de son chemin.
#[tauri::command]
pub fn file_display_name<R: Runtime>(app: tauri::AppHandle<R>, uri: String) -> Result<String, String> {
    app.state::<Files<R>>().display_name(&uri)
}

/// Notification système d'un transfert en cours (barre de progression Android).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn notify_transfer<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: i32,
    title: String,
    text: String,
    done: u64,
    total: u64,
    finished: bool,
) -> Result<(), String> {
    app.state::<Files<R>>()
        .notify_transfer(id, &title, &text, done, total, finished)
}

/// Retire la notification d'un transfert.
#[tauri::command]
pub fn clear_transfer<R: Runtime>(app: tauri::AppHandle<R>, id: i32) -> Result<(), String> {
    app.state::<Files<R>>().clear_transfer(id)
}

/// Téléverse un fichier choisi dans le sélecteur, **entièrement côté natif**.
///
/// Le webview ne voit passer aucun octet : Kotlin ouvre le `content://` et cède son descripteur,
/// le Rust lit dessus et écrit en SFTP par blocs. La progression remonte par l'event
/// `sftp://progress`, comme sur desktop. Un transfert survit donc à un rechargement de la page.
#[tauri::command]
pub async fn sftp_upload_uri<R: Runtime>(
    app: tauri::AppHandle<R>,
    core: tauri::State<'_, std::sync::Arc<minestrator_core::Core>>,
    server_id: i64,
    uri: String,
    name: String,
    remote_dir: String,
    transfer_id: String,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::FromRawFd;

        // Sonde du dossier distant avant d'ouvrir le fichier local.
        //
        // Deux raisons : réveiller une session morte pendant que l'app était en arrière-plan
        // (`Core::with_sftp` rouvre et rejoue tout seul), et vérifier que la destination existe
        // AVANT d'engager un transfert. L'erreur est remontée telle quelle : un dossier absent ou
        // des droits insuffisants ne doivent pas être maquillés en panne de session.
        core.sftp_list(server_id, &remote_dir)
            .await
            .map_err(|e| e.to_string())?;

        let opened = app.state::<Files<R>>().open_for_read(&uri)?;
        // SAFETY : `fd` vient de `detachFd()` — Kotlin en a cédé la propriété et ne le referme
        // plus. Le `File` construit ici en devient l'unique propriétaire et le ferme à sa mort.
        let std_file = unsafe { std::fs::File::from_raw_fd(opened.fd) };
        let total = if opened.size > 0 { opened.size as u64 } else { 0 };
        let file = tokio::fs::File::from_std(std_file);
        core.run_sftp_upload_stream(server_id, file, &name, total, &remote_dir, &transfer_id)
            .await;
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (&app, &core, server_id, &uri, &name, &remote_dir, &transfer_id);
        Err("Envoi par URI : Android uniquement.".into())
    }
}
