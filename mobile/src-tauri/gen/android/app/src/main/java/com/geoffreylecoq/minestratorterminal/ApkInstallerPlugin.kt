package com.geoffreylecoq.minestratorterminal

import android.app.Activity
import android.content.Intent
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class InstallArgs {
  lateinit var path: String
}

/**
 * Lance l'installeur système Android sur un APK téléchargé (auto-update in-app).
 * Appelé depuis Rust via `run_mobile_plugin("install", { path })`.
 */
@TauriPlugin
class ApkInstallerPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun install(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(InstallArgs::class.java)
      val file = File(args.path)
      val uri = FileProvider.getUriForFile(
        activity,
        activity.packageName + ".fileprovider",
        file
      )
      val intent = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, "application/vnd.android.package-archive")
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
      }
      activity.startActivity(intent)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }
}
