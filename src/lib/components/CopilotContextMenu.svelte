<script lang="ts">
  import { t } from '$lib/i18n';
  import { api } from '$lib/ipc';
  import { copilotMenu, closeCopilotMenu } from '$lib/copilot/menu.svelte';
  import Icon from './Icon.svelte';

  const menu = copilotMenu();
  let sent = $state(false);
  let closeTimer: ReturnType<typeof setTimeout> | undefined;

  async function analyze() {
    try {
      await api.copilotAnalyze(menu.serverId, menu.serverName, menu.text);
      sent = true;
      clearTimeout(closeTimer); // évite qu'un timer d'une sélection précédente ferme ce menu-ci
      closeTimer = setTimeout(() => {
        sent = false;
        closeCopilotMenu();
      }, 900);
    } catch {
      closeCopilotMenu();
    }
  }

  function copy() {
    // Copie la sélection au presse-papier (réflexe clic-droit ; complète le Ctrl+C de la console).
    navigator.clipboard.writeText(menu.text).catch(() => {});
    closeCopilotMenu();
  }

  // Position bornée à la fenêtre.
  const left = $derived(Math.min(menu.x, (globalThis.innerWidth ?? 9999) - 230));
  const top = $derived(Math.min(menu.y, (globalThis.innerHeight ?? 9999) - 90));

  function preview(text: string): string {
    const one = text.replace(/\s+/g, ' ').trim();
    return one.length > 60 ? one.slice(0, 60) + '…' : one;
  }
</script>

{#if menu.open}
  <button class="backdrop" aria-label={t('common.close')} onclick={closeCopilotMenu}></button>
  <div class="menu" style="left: {left}px; top: {top}px" role="menu">
    <button class="entry" onclick={analyze} disabled={sent}>
      <span class="ico"><Icon name="activity" size={16} /></span>
      <span>{sent ? t('copilot.analyzeSent') : t('copilot.analyze')}</span>
    </button>
    <button class="entry" onclick={copy}>
      <span class="ico"><Icon name="copy" size={16} /></span>
      <span>{t('copilot.copy')}</span>
    </button>
    <div class="hint">{preview(menu.text)}</div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 60;
    cursor: default;
  }
  .menu {
    position: fixed;
    z-index: 61;
    min-width: 200px;
    max-width: 260px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 5px;
    overflow: hidden;
  }
  .entry {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 13.5px;
    color: var(--text);
    padding: 8px 10px;
  }
  .entry:hover:not(:disabled) {
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
  }
  .entry:disabled {
    color: var(--brand-primary);
    cursor: default;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    color: var(--brand-primary);
  }
  .hint {
    font-size: 11px;
    color: var(--text-dim);
    font-family: var(--font-mono);
    padding: 4px 10px 6px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
