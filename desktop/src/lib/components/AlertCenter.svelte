<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { ago } from '$lib/copilot/format';
  import { alertItems, unreadCount, markAllRead, clearAlerts } from '$lib/alerts/alerts.svelte';
  import { api } from '$lib/ipc';
  import { tabs } from '$lib/tabs/tabs.svelte';
  import type { UserNotification } from '$lib/types';
  import Icon from './Icon.svelte';

  let open = $state(false);
  let now = $state(Date.now());
  const count = $derived(unreadCount());
  /**
   * Notifications de l'hébergeur — distinctes des alertes du superviseur (locales, temps réel).
   * Chargées à l'ouverture du panneau : elles arrivent avec le profil, donc un appel réseau.
   */
  let notifications = $state<UserNotification[]>([]);
  let notifLoaded = $state(false);

  onMount(() => {
    const iv = setInterval(() => (now = Date.now()), 30000);
    return () => clearInterval(iv);
  });

  function toggle() {
    open = !open;
    if (open) {
      markAllRead();
      void loadNotifications();
    }
  }

  /** Best-effort : le panneau reste utile (alertes locales) même si l'API ne répond pas. */
  async function loadNotifications() {
    try {
      notifications = await api.userNotifications();
    } catch {
      notifications = [];
    } finally {
      notifLoaded = true;
    }
  }

  /** Sévérité au vocabulaire du panel → token de couleur de la charte. */
  function notifColor(c: string): string {
    switch (c) {
      case 'error':
        return 'var(--state-danger)';
      case 'warning':
        return 'var(--state-pending)';
      case 'success':
        return 'var(--state-running)';
      default:
        return 'var(--brand-primary)';
    }
  }

  function color(severity: string): string {
    return severity === 'critical' ? 'var(--state-danger)' : 'var(--state-pending)';
  }
</script>

<div class="ac">
  <button class="bell" onclick={toggle} title={t('alerts.title')} aria-label={t('alerts.title')}>
    <span class="ico"><Icon name="bell" size={17} /></span>
    {#if count > 0}<span class="badge">{count > 9 ? '9+' : count}</span>{/if}
  </button>

  {#if open}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (open = false)}></button>
    <div class="panel" role="menu">
      <div class="head">
        <span class="title">{t('alerts.title')}</span>
        {#if alertItems().length > 0}
          <button class="clear" onclick={clearAlerts}>{t('alerts.clear')}</button>
        {/if}
      </div>
      <!-- UNE seule zone de défilement pour les deux sections : deux zones distinctes se
           partageraient la hauteur du panneau et masqueraient le contenu sans le signaler. -->
      <div class="scroll">
      {#if alertItems().length === 0}
        <div class="empty dim">{t('alerts.empty')}</div>
      {:else}
        <div class="list">
          {#each alertItems() as a (a.id)}
            <div class="item">
              <span class="dot" style="background: {color(a.severity)}"></span>
              <div class="body">
                <div class="top">
                  <span class="srv">{a.server_name}</span>
                  <span class="time dim">{ago(now, a.ts)}</span>
                </div>
                <div class="msg">{a.message}</div>
              </div>
            </div>
          {/each}
        </div>
      {/if}

      {#if notifications.length > 0}
        <div class="head sub">
          <span class="title">{t('alerts.fromHost')}</span>
          <span class="spacer"></span>
          <span class="count dim">{notifications.length}</span>
          <button
            class="clear"
            onclick={() => {
              open = false;
              tabs.openNotifications();
            }}>{t('alerts.seeAll')}</button
          >
        </div>
        <div class="list">
          {#each notifications as n (n.id)}
            <div class="item">
              <span class="dot" style="background: {notifColor(n.color)}"></span>
              <div class="body">
                <div class="top">
                  <span class="srv">{n.kind}</span>
                  <span class="time dim">{n.date.slice(0, 16)}</span>
                </div>
                <div class="msg">{n.message}</div>
              </div>
            </div>
          {/each}
        </div>
      {:else if notifLoaded && alertItems().length === 0}
        <div class="empty dim">{t('alerts.noHostNotif')}</div>
      {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  /* Collant : en défilant dans les notifications, on garde sous les yeux la section où l'on est. */
  .head.sub {
    position: sticky;
    top: 0;
    z-index: 1;
    gap: 8px;
    padding: 9px 14px;
    border-top: 1px solid var(--border);
    background: var(--elevated);
  }
  .spacer {
    flex: 1;
  }
  .count {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    padding: 1px 7px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }

  .ac {
    position: relative;
    flex: none;
  }
  .bell {
    position: relative;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius);
    cursor: pointer;
    padding: 5px 9px;
    color: var(--text-muted);
    font-size: 15px;
    line-height: 1;
  }
  .bell:hover {
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .ico {
    display: inline-flex;
    align-items: center;
  }
  .badge {
    position: absolute;
    top: -2px;
    right: -2px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 999px;
    background: var(--state-danger);
    color: #fff;
    font-size: 10px;
    font-weight: 700;
    display: grid;
    place-items: center;
    box-shadow: 0 0 0 2px var(--surface);
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 30;
    cursor: default;
  }
  .panel {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 31;
    width: 360px;
    /* Plafonné à la fenêtre : le panneau est ancré en haut à droite, une hauteur fixe le
       ferait déborder sous le bas de l'écran sur une petite fenêtre. */
    max-height: min(70vh, 560px);
    display: flex;
    flex-direction: column;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .title {
    font-weight: 700;
    font-size: 14px;
  }
  .clear {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
  }
  .clear:hover {
    color: var(--text);
  }
  .empty {
    padding: 32px 16px;
    text-align: center;
    font-size: 13px;
  }
  /* Le conteneur défilant unique. `overscroll-behavior` évite que la molette, arrivée en bout
     de liste, se propage à la page derrière le panneau. */
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    scrollbar-color: color-mix(in srgb, var(--text) 28%, transparent) transparent;
  }
  /* Barre toujours visible plutôt qu'au survol : c'est elle qui signale qu'il reste du contenu. */
  .scroll::-webkit-scrollbar {
    width: 10px;
  }
  .scroll::-webkit-scrollbar-thumb {
    background: color-mix(in srgb, var(--text) 24%, transparent);
    border: 3px solid transparent;
    background-clip: padding-box;
    border-radius: 999px;
  }
  .scroll::-webkit-scrollbar-thumb:hover {
    background: color-mix(in srgb, var(--text) 40%, transparent);
    background-clip: padding-box;
  }
  .item {
    display: flex;
    gap: 10px;
    padding: 11px 14px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  }
  .item:last-child {
    border-bottom: none;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    margin-top: 5px;
    flex: none;
  }
  .body {
    min-width: 0;
    flex: 1;
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }
  .srv {
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .time {
    font-size: 11px;
    flex: none;
  }
  .msg {
    font-size: 12.5px;
    color: var(--text-muted);
    margin-top: 2px;
  }
</style>
