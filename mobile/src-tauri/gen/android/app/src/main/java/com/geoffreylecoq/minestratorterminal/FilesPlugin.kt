package com.geoffreylecoq.minestratorterminal

import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import androidx.core.app.NotificationCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/** Arguments de `displayName`. */
@InvokeArg
internal class UriArgs {
  var uri: String = ""
}

/** Arguments des notifications de transfert. */
@InvokeArg
internal class TransferArgs {
  /** Identifiant stable du transfert : une notification par transfert, mise à jour en place. */
  var id: Int = 0
  var title: String = ""
  var text: String = ""
  /** Octets transférés / total. `total = 0` ⇒ barre indéterminée. */
  var done: Long = 0
  var total: Long = 0
  /** Transfert achevé : la barre disparaît et la notification devient effaçable. */
  var finished: Boolean = false
}

/**
 * Résolution du **nom réel** d'un fichier choisi via le sélecteur Android.
 *
 * Le sélecteur (SAF) renvoie un `content://` opaque — par exemple
 * `content://com.android.providers.media.documents/document/image%3A3604` — dont le dernier
 * segment n'est qu'un identifiant interne : ni le nom, ni l'extension. Seul le `ContentResolver`
 * connaît le nom d'affichage, via `OpenableColumns.DISPLAY_NAME`.
 *
 * Sans ça, un fichier envoyé arrive sur le serveur sous un nom du genre « image%3A3604 », sans
 * extension — donc plus reconnu comme image ensuite.
 */
@TauriPlugin
class FilesPlugin(private val activity: Activity) : Plugin(activity) {
  companion object {
    private const val TRANSFER_CHANNEL = "minestrator_transfers"
  }

  /**
   * Affiche ou met à jour la notification d'un transfert.
   *
   * Canal distinct de celui du superviseur : l'utilisateur peut couper les notifications de
   * transfert sans perdre les alertes serveur. Importance basse — un transfert qu'on vient de
   * lancer soi-même n'a pas à sonner.
   */
  @Command
  fun notifyTransfer(invoke: Invoke) {
    try {
      val a = invoke.parseArgs(TransferArgs::class.java)
      ensureChannel()
      val mgr = activity.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      val b = NotificationCompat.Builder(activity, TRANSFER_CHANNEL)
        .setContentTitle(a.title)
        .setContentText(a.text)
        .setSmallIcon(android.R.drawable.stat_sys_download)
        .setPriority(NotificationCompat.PRIORITY_LOW)
        .setOnlyAlertOnce(true)
      if (a.finished) {
        b.setSmallIcon(android.R.drawable.stat_sys_download_done)
        b.setOngoing(false)
        b.setAutoCancel(true)
      } else {
        b.setOngoing(true)
        if (a.total > 0) {
          // Ramené sur 100 : `setProgress` prend des Int, une taille en octets déborde.
          val pct = ((a.done * 100) / a.total).toInt().coerceIn(0, 100)
          b.setProgress(100, pct, false)
        } else {
          b.setProgress(0, 0, true)
        }
      }
      mgr.notify(a.id, b.build())
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  /** Retire la notification d'un transfert (annulation, ou nettoyage après coup). */
  @Command
  fun clearTransfer(invoke: Invoke) {
    try {
      val a = invoke.parseArgs(TransferArgs::class.java)
      val mgr = activity.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      mgr.cancel(a.id)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  private fun ensureChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val mgr = activity.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (mgr.getNotificationChannel(TRANSFER_CHANNEL) != null) return
    val channel = NotificationChannel(
      TRANSFER_CHANNEL,
      "Transferts de fichiers",
      NotificationManager.IMPORTANCE_LOW
    ).apply {
      description = "Progression des envois et téléchargements SFTP."
      setShowBadge(false)
    }
    mgr.createNotificationChannel(channel)
  }

  /**
   * Ouvre un fichier en lecture et rend son **descripteur brut** au Rust, avec sa taille.
   *
   * `detachFd()` transfère la propriété du descripteur : Kotlin ne le ferme plus, c'est au Rust
   * de le faire. C'est ce qui permet au transfert de vivre entièrement côté natif — sans faire
   * transiter les octets par le webview, dont les conversions base64 saturaient la mémoire.
   */
  @Command
  fun openForRead(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(UriArgs::class.java)
      val uri = Uri.parse(args.uri)
      val pfd = activity.contentResolver.openFileDescriptor(uri, "r")
        ?: throw IllegalStateException("fichier illisible")
      val size = pfd.statSize // -1 si la taille est inconnue (flux) : le Rust lira jusqu'au bout
      val res = JSObject()
      res.put("fd", pfd.detachFd())
      res.put("size", size)
      invoke.resolve(res)
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  @Command
  fun displayName(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(UriArgs::class.java)
      val res = JSObject()
      res.put("value", resolve(args.uri))
      invoke.resolve(res)
    } catch (ex: Exception) {
      invoke.reject(ex.message)
    }
  }

  private fun resolve(raw: String): String {
    // Chemin de fichier classique (cas non-SAF) : le dernier segment est déjà le bon nom.
    if (!raw.startsWith("content://")) return raw.substringAfterLast('/')

    val uri = Uri.parse(raw)
    activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
      ?.use { cursor ->
        val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (idx >= 0 && cursor.moveToFirst()) {
          val name = cursor.getString(idx)
          if (!name.isNullOrBlank()) return name
        }
      }
    // Repli : dernier segment décodé (`image%3A3604` → `image:3604`), nettoyé des caractères
    // interdits côté serveur. Mieux que rien, mais toujours sans extension.
    return Uri.decode(raw.substringAfterLast('/')).replace(':', '_')
  }
}
