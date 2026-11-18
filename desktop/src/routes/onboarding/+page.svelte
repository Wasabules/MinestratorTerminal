<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { setAuth } from '#lib/stores/auth.svelte.ts';
  import { accountEvents } from '#lib/events.ts';
  import { applyAccountSwitch } from '#lib/stores/accounts.svelte.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { Account } from '#lib/types.ts';

  const PANEL_URL = 'https://minestrator.com/my/account?section=api';

  let key = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  /**
   * Autres comptes déjà enregistrés : la clé du compte actif a pu être révoquée, ou ce compte
   * retiré — on peut reprendre un autre compte sans ressaisir de clé.
   */
  let others = $state<Account[]>([]);
  let unlisten: UnlistenFn | undefined;

  onMount(async () => {
    try {
      others = (await api.accountsList()).filter((a) => !a.active);
    } catch {
      others = [];
    }
    // Connexion faite depuis une autre fenêtre : celle-ci suit.
    unlisten = await accountEvents.switched((s) => {
      if (s.current) void applyAccountSwitch(s.current);
    });
  });
  onDestroy(() => unlisten?.());

  async function useAccount(a: Account) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await applyAccountSwitch(await api.accountSwitch(a.id));
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = false;
    }
  }

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

    {#if others.length > 0}
      <div class="others">
        <p class="dim foot">{t('onboarding.otherAccounts')}</p>
        <div class="other-list">
          {#each others as a (a.id)}
            <button type="button" class="btn btn--ghost" onclick={() => useAccount(a)} disabled={busy}>
              {a.pseudo}
            </button>
          {/each}
        </div>
      </div>
    {/if}

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
  .others {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .other-list {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
