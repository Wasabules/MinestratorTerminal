<script lang="ts">
  /**
   * Boîte de dialogue déclenchée par la croix de la fenêtre principale : quitter l'application ou la
   * réduire dans le tray, avec l'option « se souvenir de mon choix ». Le backend (lib.rs) empêche la
   * fermeture réelle et émet `close-requested` ; c'est ici qu'on décide de l'action selon la
   * préférence (voir close.svelte.ts). Réponse via les commandes hide_to_tray / quit_app.
   */
  import { onMount, onDestroy } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { api } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import { closeBehavior, setCloseBehavior, type CloseBehavior } from '$lib/close.svelte';
  import Icon from './Icon.svelte';

  let open = $state(false);
  let remember = $state(false);
  let un: UnlistenFn | undefined;

  onMount(async () => {
    // Seule la fenêtre principale gère la croix ; les fenêtres détachées se ferment normalement.
    if (getCurrentWindow().label !== 'main') return;
    un = await listen('close-requested', onCloseRequested);
  });
  onDestroy(() => un?.());

  function onCloseRequested() {
    switch (closeBehavior()) {
      case 'minimize':
        void api.hideToTray();
        break;
      case 'quit':
        void api.quitApp();
        break;
      default:
        remember = false;
        open = true;
    }
  }

  function apply(choice: Exclude<CloseBehavior, 'ask'>) {
    open = false;
    if (remember) setCloseBehavior(choice);
    if (choice === 'quit') void api.quitApp();
    else void api.hideToTray();
  }

  function onKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      open = false;
    } else if (e.key === 'Enter') {
      e.preventDefault();
      apply('minimize'); // choix par défaut : le plus sûr (ne perd pas la session)
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
  <button class="cd-backdrop" aria-label={t('common.cancel')} onclick={() => (open = false)}></button>
  <div class="cd-modal card" role="dialog" aria-modal="true" aria-labelledby="cd-title">
    <div class="cd-head">
      <span class="cd-ico"><Icon name="power" size={18} /></span>
      <div class="cd-text">
        <h2 id="cd-title">{t('close.title')}</h2>
        <p class="cd-body">{t('close.body')}</p>
      </div>
    </div>

    <label class="cd-remember">
      <input type="checkbox" bind:checked={remember} />
      <span>{t('close.remember')}</span>
    </label>

    <div class="cd-actions">
      <button class="btn btn--ghost" onclick={() => (open = false)}>{t('common.cancel')}</button>
      <span class="cd-spacer"></span>
      <button class="btn btn--ghost" onclick={() => apply('quit')}>{t('close.quit')}</button>
      <button class="btn" onclick={() => apply('minimize')}>{t('close.minimize')}</button>
    </div>
  </div>
{/if}

<style>
  .cd-backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
    border: none;
    cursor: default;
    background: color-mix(in srgb, #000 45%, transparent);
    backdrop-filter: blur(2px);
  }
  .cd-modal {
    position: fixed;
    z-index: 91;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(440px, calc(100vw - 40px));
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .cd-head {
    display: flex;
    gap: 13px;
    align-items: flex-start;
  }
  .cd-ico {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
  }
  .cd-text {
    min-width: 0;
  }
  h2 {
    margin: 2px 0 0;
    font-size: 16px;
    letter-spacing: -0.01em;
  }
  .cd-body {
    margin: 6px 0 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  .cd-remember {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 13px;
    color: var(--text-muted);
    cursor: pointer;
    user-select: none;
  }
  .cd-remember input {
    width: 16px;
    height: 16px;
    accent-color: var(--brand-primary);
    cursor: pointer;
  }
  .cd-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .cd-spacer {
    flex: 1;
  }
  .cd-actions .btn {
    padding: 8px 14px;
    font-size: 13px;
  }
</style>
