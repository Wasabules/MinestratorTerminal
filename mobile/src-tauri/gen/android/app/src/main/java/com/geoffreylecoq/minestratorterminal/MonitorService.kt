package com.geoffreylecoq.minestratorterminal

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * Service **au premier plan** dont l'unique rôle est de **garder le process de l'app vivant**
 * quand l'UI est fermée/en arrière-plan.
 *
 * Le superviseur Rust (démarré dans `setup()` de la lib Tauri, sur le runtime tokio du process)
 * continue alors de sonder les serveurs et `forward()` poste les notifications d'alerte — même
 * app fermée. On ne duplique donc PAS la surveillance ici : ce service n'est qu'un « maintien en vie ».
 *
 * Android exige qu'un service au premier plan affiche une notification persistante permanente :
 * c'est le petit bandeau « Surveillance des serveurs active ».
 *
 * Limite : si le système tue complètement le process (mémoire basse, balayage sur certains OEM),
 * la surveillance s'arrête jusqu'à réouverture de l'app. Pour une garantie « même tél éteint »,
 * voir le daemon + push FCM (docs/PUSH.md).
 */
class MonitorService : Service() {
  companion object {
    private const val CHANNEL_ID = "minestrator_monitor"
    private const val NOTIF_ID = 4201

    fun start(context: Context) {
      val intent = Intent(context, MonitorService::class.java)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        context.startForegroundService(intent)
      } else {
        context.startService(intent)
      }
    }

    fun stop(context: Context) {
      context.stopService(Intent(context, MonitorService::class.java))
    }
  }

  override fun onCreate() {
    super.onCreate()
    createChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val notif = buildNotification()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      // Android 14+ : un type de service au premier plan est obligatoire.
      startForeground(NOTIF_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
    } else {
      startForeground(NOTIF_ID, notif)
    }
    // START_STICKY : si le système récupère de la mémoire, il tentera de recréer le service.
    return START_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  private fun createChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (mgr.getNotificationChannel(CHANNEL_ID) != null) return
    val channel = NotificationChannel(
      CHANNEL_ID,
      "Surveillance en arrière-plan",
      NotificationManager.IMPORTANCE_LOW // discret : pas de son ni de vibration pour le bandeau permanent
    ).apply {
      description = "Maintient la surveillance des serveurs active quand l'app est fermée."
      setShowBadge(false)
    }
    mgr.createNotificationChannel(channel)
  }

  private fun buildNotification(): Notification {
    // Tap → rouvre l'app.
    val launch = packageManager.getLaunchIntentForPackage(packageName)
    val flags = PendingIntent.FLAG_UPDATE_CURRENT or
      (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0)
    val pending = launch?.let { PendingIntent.getActivity(this, 0, it, flags) }

    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setContentTitle("MinestratorTerminal")
      .setContentText("Surveillance des serveurs active")
      .setSmallIcon(android.R.drawable.stat_notify_sync)
      .setOngoing(true)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setContentIntent(pending)
      .build()
  }
}
