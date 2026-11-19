<script lang="ts">
  /**
   * Secrets des agents (Réglages → MCP) : valeurs (token de bot, clé d'API, mot de passe RCON…)
   * qu'un agent MCP écrit dans les configs des serveurs par une référence, sans jamais les voir.
   * La valeur saisie part au cœur, qui la range dans le stockage sécurisé : l'UI ne la relit
   * jamais (champ masqué, oublié après l'envoi). « Générer » la fait tirer au hasard par le cœur,
   * sans qu'elle passe par ici. Logique et règles : crates/minestrator-core/src/agent_secrets.rs.
   */
  import { onMount, onDestroy } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { fmtDate } from '#lib/automations/format.ts';
  import type { AgentSecret, SecretCharset } from '#lib/types.ts';
  import Icon from '../Icon.svelte';

  /** Bornes du cœur (`agent_secrets.rs`), rappelées pour guider la saisie ; le cœur fait foi. */
  const NAME_MAX = 64;
  const VALUE_MIN = 8;
  const NAME_RE = /^[a-z0-9_-]+$/;
  const LENGTHS = [16, 24, 32, 48, 64];
  const CHARSETS: { value: SecretCharset; label: string }[] = [
    { value: 'alphanumeric', label: 'agentSecrets.charsetAlphanumeric' },
    { value: 'hex', label: 'agentSecrets.charsetHex' },
    { value: 'url_safe', label: 'agentSecrets.charsetUrlSafe' },
    { value: 'symbols', label: 'agentSecrets.charsetSymbols' },
  ];

  interface Draft {
    name: string;
    description: string;
    source: 'enter' | 'generate';
    /** Valeur saisie : jamais pré-remplie, oubliée dès l'enregistrement. */
    value: string;
    length: number;
    charset: SecretCharset;
  }

  let list = $state<AgentSecret[]>([]);
  let loaded = $state(false);
  let draft = $state<Draft | null>(null);
  let busy = $state<string | null>(null);
  let message = $state<{ ok: boolean; text: string } | null>(null);
  let msgTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(load);
  onDestroy(() => clearTimeout(msgTimer));

  /** Référence telle que l'écrit un agent. */
  const ref = (name: string) => `{{secret:${name}}}`;

  async function load() {
    try {
      list = await api.agentSecrets();
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      loaded = true;
    }
  }

  function say(ok: boolean, text: string) {
    message = { ok, text };
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => (message = null), 4000);
  }

  function startNew() {
    draft = {
      name: '',
      description: '',
      source: 'enter',
      value: '',
      length: 32,
      charset: 'alphanumeric',
    };
  }

  const nameError = $derived.by(() => {
    if (!draft || draft.name === '') return null;
    if (draft.name.length > NAME_MAX || !NAME_RE.test(draft.name)) return t('agentSecrets.nameInvalid');
    if (list.some((s) => s.name === draft!.name)) return t('agentSecrets.nameTaken');
    return null;
  });

  const valueOk = $derived(
    !!draft && (draft.source === 'generate' || draft.value.trim().length >= VALUE_MIN),
  );

  const draftValid = $derived(!!draft && draft.name !== '' && nameError === null && valueOk);

  async function save() {
    if (!draft || !draftValid) return;
    busy = 'save';
    const { name, description, source, value, length, charset } = draft;
    try {
      if (source === 'generate') {
        await api.generateAgentSecret(name, description.trim(), length, charset);
      } else {
        await api.addAgentSecret(name, description.trim(), value);
      }
      draft = null;
      say(true, t('agentSecrets.saved'));
      await load();
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      busy = null;
    }
  }

  async function copyRef(name: string) {
    try {
      await navigator.clipboard.writeText(ref(name));
      say(true, t('agentSecrets.copied'));
    } catch (e) {
      say(false, String(e));
    }
  }

  async function remove(s: AgentSecret) {
    const ok = await confirm(t('agentSecrets.confirmDelete', { name: s.name }), { kind: 'warning' });
    if (!ok) return;
    busy = s.name;
    try {
      await api.deleteAgentSecret(s.name);
      say(true, t('agentSecrets.deleted'));
      await load();
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      busy = null;
    }
  }
</script>

<h2>{t('agentSecrets.title')}</h2>
<p class="desc dim">{t('agentSecrets.desc')}</p>
<p class="note">
  <Icon name="key" size={14} />
  <span>{t('agentSecrets.syntax', { ref: ref(t('agentSecrets.namePlaceholder')) })}</span>
</p>

{#if loaded && list.length === 0 && !draft}
  <p class="desc dim">{t('agentSecrets.empty')}</p>
{/if}

{#if list.length > 0}
  <div class="list">
    {#each list as s (s.name)}
      <div class="row">
        <div class="main">
          <div class="top">
            <code class="ref">{ref(s.name)}</code>
            {#if s.generated}<span class="tag">{t('agentSecrets.generated')}</span>{/if}
            <span class="date dim">{t('agentSecrets.created', { date: fmtDate(s.created_at) })}</span>
          </div>
          {#if s.description}<div class="what dim">{s.description}</div>{/if}
        </div>
        <div class="acts">
          <button
            class="mini"
            title={t('agentSecrets.copyRef')}
            aria-label={t('agentSecrets.copyRef')}
            onclick={() => copyRef(s.name)}
          >
            <Icon name="copy" size={13} />
          </button>
          <button
            class="mini danger"
            title={t('agentSecrets.delete')}
            aria-label={t('agentSecrets.delete')}
            disabled={busy !== null}
            onclick={() => remove(s)}
          >
            {#if busy === s.name}<span class="spinner"></span>{:else}<Icon name="trash" size={13} />{/if}
          </button>
        </div>
      </div>
    {/each}
  </div>
{/if}

{#if draft}
  <div class="editor">
    <div class="ed-title">{t('agentSecrets.newSecret')}</div>
    <label class="field">
      <span class="lab">{t('agentSecrets.name')}</span>
      <!-- Les noms sont en minuscules : la saisie est convertie au fil de l'eau. -->
      <input
        class="txt mono"
        bind:value={() => draft!.name, (v) => (draft!.name = v.toLowerCase())}
        maxlength={NAME_MAX}
        autocomplete="off"
        spellcheck="false"
        placeholder={t('agentSecrets.namePlaceholder')}
      />
      {#if nameError}
        <span class="err">{nameError}</span>
      {:else}
        <span class="hint dim">{t('agentSecrets.nameHint', { max: NAME_MAX })}</span>
      {/if}
    </label>
    <label class="field">
      <span class="lab">{t('agentSecrets.description')}</span>
      <input
        class="txt"
        bind:value={draft.description}
        maxlength="200"
        placeholder={t('agentSecrets.descriptionPlaceholder')}
      />
    </label>
    <div class="field">
      <span class="lab">{t('agentSecrets.source')}</span>
      <div class="seg" role="radiogroup" aria-label={t('agentSecrets.source')}>
        <button
          role="radio"
          aria-checked={draft.source === 'enter'}
          class:on={draft.source === 'enter'}
          onclick={() => (draft!.source = 'enter')}
        >
          {t('agentSecrets.sourceEnter')}
        </button>
        <button
          role="radio"
          aria-checked={draft.source === 'generate'}
          class:on={draft.source === 'generate'}
          onclick={() => {
            draft!.source = 'generate';
            draft!.value = '';
          }}
        >
          {t('agentSecrets.sourceGenerate')}
        </button>
      </div>
    </div>
    {#if draft.source === 'enter'}
      <label class="field">
        <span class="lab">{t('agentSecrets.value')}</span>
        <!-- Champ masqué : la valeur part au stockage sécurisé, elle n'est jamais réaffichée. -->
        <input
          class="txt"
          type="password"
          autocomplete="off"
          spellcheck="false"
          bind:value={draft.value}
          placeholder={t('agentSecrets.valuePlaceholder')}
        />
        {#if draft.value !== '' && !valueOk}
          <span class="err">{t('agentSecrets.valueTooShort', { min: VALUE_MIN })}</span>
        {/if}
      </label>
    {:else}
      <div class="gen">
        <label class="field">
          <span class="lab">{t('agentSecrets.length')}</span>
          <select class="txt" bind:value={draft.length}>
            {#each LENGTHS as n (n)}
              <option value={n}>{n}</option>
            {/each}
          </select>
        </label>
        <label class="field">
          <span class="lab">{t('agentSecrets.charset')}</span>
          <select class="txt" bind:value={draft.charset}>
            {#each CHARSETS as c (c.value)}
              <option value={c.value}>{t(c.label)}</option>
            {/each}
          </select>
        </label>
      </div>
      <p class="hint dim">{t('agentSecrets.generateHint')}</p>
    {/if}
    <div class="ed-actions">
      <span class="spacer"></span>
      <button class="btn btn--ghost" onclick={() => (draft = null)} disabled={busy === 'save'}>
        {t('agentSecrets.cancel')}
      </button>
      <button class="btn" onclick={save} disabled={!draftValid || busy !== null}>
        {#if busy === 'save'}<span class="spinner"></span>{/if}
        {busy === 'save' ? t('agentSecrets.saving') : t('agentSecrets.save')}
      </button>
    </div>
  </div>
{:else}
  <div class="actions">
    <button class="btn btn--ghost" onclick={startNew}>
      <Icon name="plus" size={14} />
      {t('agentSecrets.add')}
    </button>
  </div>
{/if}

{#if message}
  <p class="msg" class:ok={message.ok}>{message.text}</p>
{/if}

<style>
  h2 {
    margin: 26px 0 12px;
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
  }
  .desc {
    font-size: 13px;
    margin: 0 0 10px;
    line-height: 1.55;
  }
  .note {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 12.5px;
    line-height: 1.5;
    margin: 0 0 14px;
    padding: 9px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-muted);
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--border);
  }
  .row:last-child {
    border-bottom: none;
  }
  .main {
    flex: 1;
    min-width: 0;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .ref {
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: var(--text);
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .tag {
    font-size: 10.5px;
    font-weight: 800;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 6px;
  }
  .date {
    font-size: 11.5px;
  }
  .what {
    font-size: 12px;
    margin-top: 3px;
    line-height: 1.45;
  }
  .acts {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .mini {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 5px 9px;
  }
  .mini:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--text-dim);
  }
  .mini:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .mini.danger:hover:not(:disabled) {
    color: var(--state-danger);
  }
  .editor {
    margin-top: 14px;
    padding: 14px 16px;
    border: 1px solid color-mix(in srgb, var(--brand-primary) 45%, var(--border));
    border-radius: var(--radius-lg);
    background: var(--surface);
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .ed-title {
    font-weight: 800;
    font-size: 14px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .lab {
    font-size: 12.5px;
    font-weight: 800;
    color: var(--text-muted);
  }
  .txt {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 11px;
  }
  .txt.mono {
    font-family: var(--font-mono);
  }
  .txt:focus {
    outline: none;
    border-color: var(--brand-primary);
  }
  .seg {
    display: inline-flex;
    align-self: flex-start;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .seg button {
    background: none;
    border: none;
    border-right: 1px solid var(--border);
    color: var(--text-muted);
    font: inherit;
    font-size: 12.5px;
    padding: 7px 12px;
    cursor: pointer;
  }
  .seg button:last-child {
    border-right: none;
  }
  .seg button.on {
    background: color-mix(in srgb, var(--brand-primary) 18%, transparent);
    color: var(--text);
    font-weight: 800;
  }
  .gen {
    display: grid;
    grid-template-columns: minmax(0, 140px) minmax(0, 1fr);
    gap: 12px;
  }
  .hint {
    font-size: 12px;
    margin: 0;
    line-height: 1.45;
  }
  .err {
    font-size: 12px;
    color: var(--state-danger);
  }
  .ed-actions,
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
  }
  .actions {
    margin-top: 12px;
  }
  .spacer {
    flex: 1;
  }
  .msg {
    font-size: 13px;
    margin: 10px 0 0;
    color: var(--state-danger);
  }
  .msg.ok {
    color: var(--state-running);
  }
  @media (max-width: 560px) {
    .gen {
      grid-template-columns: 1fr;
    }
  }
</style>
