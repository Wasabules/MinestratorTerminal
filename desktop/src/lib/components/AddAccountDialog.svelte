<script lang="ts">
  /**
   * « Ajouter un compte » : saisie d'une clé API d'un autre compte MineStrator. Le cœur la valide
   * auprès de l'API (doublon refusé) puis la range dans le stockage sécurisé ; elle ne revient
   * jamais au JS. Par défaut, on bascule ensuite sur le compte ajouté.
   */
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { applyAccountSwitch, refreshAccounts } from '#lib/stores/accounts.svelte.ts';
  import Icon from './Icon.svelte';

  let { onclose }: { onclose: () => void } = $props();

  const PANEL_URL = 'https://minestrator.com/my/account?section=api';

  let key = $state('');
  let switchTo = $state(true);
  let busy = $state(false);
  let err = $state<string | null>(null);

  async function submit(event: Event) {
    event.preventDefault();
    if (busy || !key.trim()) return;
    busy = true;
    err = null;
    try {
      const added = await api.accountAdd(key.trim());
      key = '';
      const current = added.active
        ? added
        : switchTo
          ? await api.accountSwitch(added.id)
          : null;
      // Fermée AVANT le réalignement : le workspace (et ce menu) vont être remontés.
      onclose();
      if (current) await applyAccountSwitch(current);
      else await refreshAccounts();
    } catch (e) {
      err = humanizeError(e);
    } finally {
      busy = false;
    }
  }

  async function openPanel() {
    try {
      await openUrl(PANEL_URL);
    } catch {
      /* openUrl indisponible : l'URL reste accessible depuis le panel */
    }
  }
</script>

<button class="aa-backdrop" aria-label={t('common.cancel')} onclick={onclose} disabled={busy}></button>
<div class="aa-modal card" role="dialog" aria-modal="true" aria-labelledby="aa-title">
  <form class="aa-form" onsubmit={submit}>
    <div class="aa-head">
      <span class="aa-ico"><Icon name="users" size={18} /></span>
      <div>
        <h2 id="aa-title">{t('account.addTitle')}</h2>
        <p class="aa-body">{t('account.addIntro')}</p>
      </div>
    </div>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      class="input"
      type="password"
      autocomplete="off"
      spellcheck="false"
      placeholder={t('onboarding.apiKeyPlaceholder')}
      bind:value={key}
      disabled={busy}
      autofocus
    />
    <label class="aa-check">
      <input type="checkbox" bind:checked={switchTo} disabled={busy} />
      <span>{t('account.switchAfterAdd')}</span>
    </label>
    <p class="aa-hint">
      {t('account.addStored')}
      <button type="button" class="linklike" onclick={openPanel}>{t('onboarding.openPanel')}</button>
    </p>
    {#if err}<p class="aa-err" role="alert">{err}</p>{/if}
    <div class="aa-actions">
      <button type="button" class="btn btn--ghost" onclick={onclose} disabled={busy}>
        {t('common.cancel')}
      </button>
      <button type="submit" class="btn" disabled={busy || !key.trim()}>
        {#if busy}<span class="spinner"></span> {t('onboarding.verifying')}{:else}{t('account.add')}{/if}
      </button>
    </div>
  </form>
</div>

<style>
  .aa-backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
    border: none;
    cursor: default;
    background: color-mix(in srgb, #000 45%, transparent);
    backdrop-filter: blur(2px);
  }
  .aa-modal {
    position: fixed;
    z-index: 91;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(460px, calc(100vw - 40px));
    padding: 20px;
  }
  .aa-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .aa-head {
    display: flex;
    gap: 13px;
    align-items: flex-start;
  }
  .aa-ico {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
  }
  h2 {
    margin: 2px 0 0;
    font-size: 16px;
    letter-spacing: -0.01em;
  }
  .aa-body {
    margin: 6px 0 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  .aa-check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .aa-hint {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .linklike {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: var(--brand-primary);
    cursor: pointer;
    text-decoration: underline;
  }
  .aa-err {
    margin: 0;
    font-size: 13px;
    color: var(--state-danger);
  }
  .aa-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
