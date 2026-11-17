<script lang="ts">
  /**
   * Paramètres du serveur : `server.properties` lu et modifié via l'API MineStrator
   * (`GET/PATCH /server/{id}/properties`), sans SFTP. L'API ne modifie qu'UNE clé par appel et
   * n'applique les changements qu'au prochain démarrage : chaque ligne s'enregistre seule, et un
   * bandeau propose de redémarrer une fois quelque chose enregistré.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  /** Valeurs connues côté serveur (référence) et valeurs en cours d'édition. */
  let original = $state<Record<string, string>>({});
  let draft = $state<Record<string, string>>({});
  let loading = $state(true);
  let error = $state<string | null>(null);
  let filter = $state('');
  let saving = $state<string | null>(null);
  let rowError = $state<Record<string, string>>({});
  /** Au moins un réglage enregistré depuis l'ouverture : effet au prochain démarrage seulement. */
  let pendingRestart = $state(false);
  let restarting = $state(false);

  const total = $derived(Object.keys(original).length);
  const keys = $derived(
    Object.keys(original).filter((k) => k.toLowerCase().includes(filter.trim().toLowerCase()))
  );

  onMount(load);

  async function load() {
    loading = true;
    error = null;
    try {
      const p = await api.serverProperties(serverId);
      original = { ...p.properties };
      draft = { ...p.properties };
      rowError = {};
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  function isDirty(key: string): boolean {
    return draft[key] !== original[key];
  }

  /** Booléen tel qu'écrit dans le fichier : rendu en case à cocher. */
  function isBool(value: string | undefined): boolean {
    return value === 'true' || value === 'false';
  }

  /** Réglage secret (mot de passe RCON…) : saisie masquée. */
  function isSecret(key: string): boolean {
    return /password|secret|token/i.test(key);
  }

  async function save(key: string) {
    if (saving || !isDirty(key)) return;
    saving = key;
    const { [key]: _, ...others } = rowError;
    rowError = others;
    try {
      await api.setServerProperty(serverId, key, draft[key]);
      original = { ...original, [key]: draft[key] };
      pendingRestart = true;
    } catch (e) {
      rowError = { ...rowError, [key]: humanizeError(e) };
    } finally {
      saving = null;
    }
  }

  function revert(key: string) {
    draft[key] = original[key];
  }

  async function restart() {
    restarting = true;
    try {
      await api.powerAction(serverId, 'restart');
      pendingRestart = false;
    } catch (e) {
      error = humanizeError(e);
    } finally {
      restarting = false;
    }
  }
</script>

<div class="props">
  <header class="head">
    <div>
      <h1>{t('props.title')}</h1>
      <p class="dim">{t('props.subtitle')}</p>
    </div>
    <button
      class="btn btn--ghost"
      onclick={load}
      disabled={loading}
      aria-label={t('common.refresh')}
      title={t('common.refresh')}
    >
      {#if loading}<span class="spinner"></span>{/if}
      <Icon name="refresh-cw" size={15} />
    </button>
  </header>

  {#if pendingRestart}
    <div class="card notice" role="status">
      <Icon name="info" size={16} />
      <span>{t('props.pendingRestart')}</span>
      <button class="btn" onclick={restart} disabled={restarting}>
        {#if restarting}<span class="spinner"></span>{/if}
        {t('props.restartNow')}
      </button>
    </div>
  {/if}

  {#if error}
    <div class="state">
      <p class="alert">{error}</p>
      <button class="btn" onclick={load}>{t('common.retry')}</button>
    </div>
  {:else if loading && total === 0}
    <div class="state"><span class="spinner"></span></div>
  {:else if total === 0}
    <p class="dim empty">{t('props.none')}</p>
  {:else}
    <div class="filter">
      <Icon name="search" size={14} />
      <input
        class="input"
        bind:value={filter}
        placeholder={t('props.filter', { count: total })}
        spellcheck="false"
      />
    </div>

    {#if keys.length === 0}
      <p class="dim empty">{t('props.noMatch')}</p>
    {:else}
      <div class="list">
        {#each keys as key (key)}
          <div class="row" class:dirty={isDirty(key)}>
            <label class="k" for={`prop-${key}`}>{key}</label>
            <div class="v">
              {#if isBool(original[key])}
                <input
                  id={`prop-${key}`}
                  type="checkbox"
                  checked={draft[key] === 'true'}
                  onchange={(e) => (draft[key] = e.currentTarget.checked ? 'true' : 'false')}
                />
              {:else}
                <input
                  id={`prop-${key}`}
                  class="input"
                  type={isSecret(key) ? 'password' : 'text'}
                  bind:value={draft[key]}
                  spellcheck="false"
                  autocomplete="off"
                  onkeydown={(e) => e.key === 'Enter' && save(key)}
                />
              {/if}
            </div>
            <div class="acts">
              {#if isDirty(key)}
                <button
                  class="btn btn--ghost"
                  onclick={() => revert(key)}
                  disabled={saving === key}
                  aria-label={t('props.revert')}
                  title={t('props.revert')}
                >
                  <Icon name="x" size={14} />
                </button>
                <button class="btn" onclick={() => save(key)} disabled={saving !== null}>
                  {#if saving === key}<span class="spinner"></span>{/if}
                  {t('props.save')}
                </button>
              {/if}
            </div>
            {#if rowError[key]}<p class="alert row-err">{rowError[key]}</p>{/if}
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .props {
    max-width: 780px;
    margin: 0 auto;
    padding: 26px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
  }
  .head h1 {
    margin: 0;
    font-size: 24px;
    letter-spacing: -0.02em;
  }
  .head p {
    margin: 5px 0 0;
    font-size: 13px;
    max-width: 60ch;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    font-size: 13px;
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
  }
  .notice span {
    flex: 1;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 28px 0;
  }
  .empty {
    font-size: 13px;
    margin: 6px 0;
  }
  .filter {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-dim);
  }
  .filter .input {
    flex: 1;
  }
  .list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(150px, 38%) 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 7px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .row:last-child {
    border-bottom: none;
  }
  .row.dirty {
    background: color-mix(in srgb, var(--brand-primary) 7%, transparent);
  }
  .k {
    font-family: var(--font-mono);
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .v {
    min-width: 0;
    display: flex;
    align-items: center;
  }
  .v .input {
    width: 100%;
    font-family: var(--font-mono);
    font-size: 12.5px;
  }
  .acts {
    display: flex;
    gap: 6px;
    min-height: 30px;
    align-items: center;
  }
  .row-err {
    grid-column: 1 / -1;
    margin: 2px 0 0;
    font-size: 12px;
  }
  @media (max-width: 560px) {
    .row {
      grid-template-columns: 1fr auto;
    }
    .k {
      grid-column: 1 / -1;
    }
  }
</style>
