<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { ago } from '$lib/copilot/format';
  import { alertItems, unreadCount, markAllRead, clearAlerts } from '$lib/alerts/alerts.svelte';
  import Icon from './Icon.svelte';

  let open = $state(false);
  let now = $state(Date.now());
  const count = $derived(unreadCount());

  onMount(() => {
    const iv = setInterval(() => (now = Date.now()), 30000);
    return () => clearInterval(iv);
  });

  function toggle() {
    open = !open;
    if (open) markAllRead();
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
    </div>
  {/if}
</div>

<style>
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
    width: 340px;
    max-height: 420px;
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
  .list {
    overflow-y: auto;
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
