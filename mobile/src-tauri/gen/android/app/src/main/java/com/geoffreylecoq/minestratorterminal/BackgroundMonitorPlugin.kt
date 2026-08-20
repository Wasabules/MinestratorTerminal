package com.geoffreylecoq.minestratorterminal

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * Surveillance en arrière-plan + **fiabilité anti-kill**.
 *
 * - `start`/`stop` : démarre/arrête le [MonitorService] (service au premier plan qui garde le
 *   process vivant pour les alertes app fermée).
 * - `isBatteryUnrestricted`/`requestBatteryUnrestricted` : état + demande d'exemption de
 *   l'optimisation batterie (sans quoi l'OS finit par tuer le process en arrière-plan).
 * - `openAppSettings` : ouvre la fiche « infos de l'app » (batterie « sans restriction », autostart
 *   selon l'OEM, notifications).
 *
 * Appelé depuis Rust via `run_mobile_plugin("<command>", …)`.
 */
/** Arguments de `updateStatus` (désérialisés par le pont Tauri). */
@InvokeArg
internal class StatusArgs {
  var title: String = "MinestratorTerminal"
  var text: String = ""
}

@TauriPlugin
class BackgroundMonitorPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun start(invoke: Invoke) {
    try {
      MonitorService.start(activity)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  @Command
  fun stop(invoke: Invoke) {
    try {
      MonitorService.stop(activity)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  /**
   * Met à jour le texte de la notification persistante (résumé de l'état des serveurs poussé par
   * le superviseur Rust). Sans effet si la surveillance en arrière-plan n'est pas active.
   */
  @Command
  fun updateStatus(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(StatusArgs::class.java)
      MonitorService.update(activity, args.title, args.text)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  /** `{ value: true }` si l'app est déjà exemptée d'optimisation batterie. */
  @Command
  fun isBatteryUnrestricted(invoke: Invoke) {
    try {
      val pm = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
      val ok = pm.isIgnoringBatteryOptimizations(activity.packageName)
      val res = JSObject()
      res.put("value", ok)
      invoke.resolve(res)
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  /** Ouvre la boîte de dialogue système « autoriser à tourner en arrière-plan ? ». */
  @Command
  fun requestBatteryUnrestricted(invoke: Invoke) {
    try {
      val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
        data = Uri.parse("package:${activity.packageName}")
      }
      activity.startActivity(intent)
      invoke.resolve()
    } catch (ex: Exception) {
      // Repli : liste complète des optimisations batterie (sans ciblage de l'app).
      try {
        activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
        invoke.resolve()
      } catch (ex2: Exception) {
        invoke.reject(ex2.message)
      }
    }
  }

  /** Ouvre la fiche « infos de l'app » (batterie sans restriction, autostart OEM, notifications). */
  @Command
  fun openAppSettings(invoke: Invoke) {
    try {
      val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
        data = Uri.fromParts("package", activity.packageName, null)
      }
      activity.startActivity(intent)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }
}
