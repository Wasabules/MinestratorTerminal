<script lang="ts">
  // Bandeau de mise à jour non intrusif. Vérifie au démarrage (dans la fenêtre PRINCIPALE seulement,
  // pour éviter des doublons dans les fenêtres détachées) ; propose l'installation, refusable.
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import type { Update } from '@tauri-apps/plugin-updater';
  import { checkForUpdate, applyUpdate, isAutoUpdateEnabled } from '$lib/updater';
  import { t } from '$lib/i18n';

  let update = $state<Update | null>(null);
  let busy = $state(false);
  let error = $state('');
  let dismissed = $state(false);

  onMount(async () => {
    if (getCurrentWindow().label !== 'main') return; // une seule fenêtre vérifie
    if (!isAutoUpdateEnabled()) return; // désactivé dans les Réglages (la vérif manuelle reste possible)
    const u = await checkForUpdate();
    if (u) update = u;
  });

  async function doUpdate() {
    if (!update) return;
    busy = true;
    error = '';
    try {
      await applyUpdate(update); // télécharge + installe + relance (ne revient pas si succès)
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      busy = false;
    }
  }
</script>

{#if update && !dismissed}
  <div class="upd" role="status" aria-live="polite">
    <span class="glow"></span>
    <div class="txt">
      <strong>{t('update.available', { v: update.version })}</strong>
      {#if error}<span class="err">{error}</span>{/if}
    </div>
    <div class="acts">
      {#if busy}
        <span class="installing"><span class="spinner"></span>{t('update.installing')}</span>
      {:else}
        <button class="ubtn primary" onclick={doUpdate}>{t('update.install')}</button>
        <button class="ubtn ghost" onclick={() => (dismissed = true)}>{t('update.later')}</button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .upd {
    position: fixed;
    right: 18px;
    bottom: 18px;
    z-index: 60;
    display: flex;
    align-items: center;
    gap: 14px;
    max-width: min(440px, calc(100vw - 36px));
    padding: 13px 16px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg, 12px);
    box-shadow: var(--shadow, 0 12px 34px rgba(0, 0, 0, 0.35));
  }
  .glow {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--brand-primary);
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--brand-primary) 20%, transparent);
    flex: none;
  }
  .txt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .txt strong {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--text);
  }
  .err {
    font-size: 12px;
    color: var(--state-danger);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: auto;
    flex: none;
  }
  .ubtn {
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    border-radius: 8px;
    padding: 7px 14px;
    cursor: pointer;
    border: 1px solid transparent;
  }
  .ubtn.primary {
    background: var(--brand-primary);
    color: #fff;
  }
  .ubtn.primary:hover {
    filter: brightness(1.08);
  }
  .ubtn.ghost {
    background: none;
    border-color: var(--border);
    color: var(--text-muted);
  }
  .ubtn.ghost:hover {
    color: var(--text);
  }
  .installing {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--text-muted);
  }
</style>
