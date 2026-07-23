<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, humanizeError } from '$lib/ipc';
  import { setAuth } from '$lib/stores/auth.svelte';
  import { t } from '$lib/i18n';

  const PANEL_URL = 'https://minestrator.com/my/account?section=api';

  let key = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: Event) {
    event.preventDefault();
    if (busy || key.trim().length === 0) return;
    busy = true;
    error = null;
    try {
      const user = await api.validateAndStoreKey(key.trim());
      setAuth({ status: 'signed_in', user });
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = false;
    }
  }

  async function openPanel() {
    try {
      await openUrl(PANEL_URL);
    } catch {
      /* openUrl indisponible : l'URL reste copiable manuellement */
    }
  }
</script>

<main class="wrap">
  <section class="card panel">
    <div class="head">
      <div class="mark" aria-hidden="true">◧</div>
      <h1>Minestrator <span class="gradient-text">Terminal</span></h1>
      <p class="muted sub">{t('onboarding.subtitle')}</p>
    </div>

    <form onsubmit={submit} class="form">
      <div class="field">
        <label class="label" for="apikey">{t('onboarding.apiKey')}</label>
        <input
          id="apikey"
          class="input"
          type="password"
          autocomplete="off"
          spellcheck="false"
          placeholder={t('onboarding.apiKeyPlaceholder')}
          bind:value={key}
          disabled={busy}
        />
      </div>

      {#if error}
        <p class="alert" role="alert">{error}</p>
      {/if}

      <button class="btn btn--block" type="submit" disabled={busy || key.trim().length === 0}>
        {#if busy}<span class="spinner"></span> {t('onboarding.verifying')}{:else}{t('onboarding.connect')}{/if}
      </button>
    </form>

    <p class="dim foot">
      {t('onboarding.noKey')}
      <button type="button" class="linklike" onclick={openPanel}>{t('onboarding.openPanel')}</button>
    </p>
  </section>
</main>

<style>
  .wrap {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .panel {
    width: 100%;
    max-width: 440px;
    padding: 36px 32px 28px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .mark {
    width: 46px;
    height: 46px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    font-size: 24px;
    color: #fff;
    background: var(--brand-gradient);
    box-shadow: var(--shadow);
  }
  h1 {
    margin: 4px 0 0;
    font-size: 26px;
    letter-spacing: -0.02em;
  }
  .sub {
    margin: 0;
    font-size: 14px;
    max-width: 40ch;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .foot {
    margin: 0;
    font-size: 13px;
  }
  .linklike {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: var(--brand-primary);
    cursor: pointer;
  }
  .linklike:hover {
    text-decoration: underline;
  }
</style>
