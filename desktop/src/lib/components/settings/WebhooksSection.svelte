<script lang="ts">
  /**
   * Réglages « Alertes externes » : destinations Discord / JSON des alertes et des résultats
   * d'automatisations. L'URL d'un webhook est un SECRET (elle suffit à poster dans le salon) :
   * saisie ici, envoyée au cœur qui la range dans le stockage sécurisé, jamais réaffichée — l'UI
   * ne connaît que sa présence (`has_url`). Envois : crates/minestrator-core/src/webhook.rs.
   */
  import { onMount, onDestroy } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { WebhookDestination, WebhookEvent, WebhookInfo, WebhookKind } from '#lib/types.ts';
  import Icon from '../Icon.svelte';

  const EVENTS: WebhookEvent[] = [
    'crash',
    'auto_restart',
    'threshold',
    'mybox',
    'task_success',
    'task_failure',
  ];

  interface Draft {
    id: string;
    name: string;
    kind: WebhookKind;
    enabled: boolean;
    events: WebhookEvent[];
    /** Nouvelle URL saisie (vide en modification = conserver l'actuelle). */
    url: string;
    hasUrl: boolean;
  }

  let list = $state<WebhookInfo[]>([]);
  let loaded = $state(false);
  let draft = $state<Draft | null>(null);
  let busy = $state<string | null>(null);
  let message = $state<{ ok: boolean; text: string } | null>(null);
  let msgTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(load);
  onDestroy(() => clearTimeout(msgTimer));

  async function load() {
    try {
      list = await api.listWebhooks();
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
      id: '',
      name: '',
      kind: 'discord',
      enabled: true,
      events: ['crash', 'auto_restart', 'task_failure'],
      url: '',
      hasUrl: false,
    };
  }

  function startEdit(w: WebhookInfo) {
    draft = { ...w.destination, events: [...w.destination.events], url: '', hasUrl: w.has_url };
  }

  /** Contrôle de forme côté UI (le cœur revalide, c'est lui qui fait foi). */
  function urlLooksValid(kind: WebhookKind, url: string): boolean {
    const u = url.trim();
    if (kind === 'discord') {
      return /^https:\/\/(ptb\.|canary\.)?(discord|discordapp)\.com\/api\/webhooks\/\S+$/.test(u);
    }
    return /^https?:\/\/\S+$/.test(u);
  }

  const draftValid = $derived(
    !!draft &&
      draft.name.trim() !== '' &&
      draft.events.length > 0 &&
      (draft.url.trim() === '' ? draft.hasUrl : urlLooksValid(draft.kind, draft.url)),
  );

  function toggleEvent(e: WebhookEvent) {
    if (!draft) return;
    draft.events = draft.events.includes(e)
      ? draft.events.filter((x) => x !== e)
      : EVENTS.filter((x) => x === e || draft!.events.includes(x));
  }

  async function save() {
    if (!draft || !draftValid) return;
    busy = 'save';
    const dest: WebhookDestination = {
      id: draft.id,
      name: draft.name.trim(),
      kind: draft.kind,
      enabled: draft.enabled,
      events: draft.events,
    };
    try {
      await api.saveWebhook(dest, draft.url.trim() || null);
      draft = null;
      say(true, t('webhooks.saved'));
      await load();
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      busy = null;
    }
  }

  /** Test : l'URL en cours de saisie si elle est remplie, sinon celle enregistrée. */
  async function test(kind: WebhookKind, url: string | null, id: string | null, key: string) {
    busy = key;
    try {
      await api.testWebhook(kind, url, id);
      say(true, t('webhooks.testOk'));
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      busy = null;
    }
  }

  async function toggleEnabled(w: WebhookInfo) {
    busy = w.destination.id;
    try {
      await api.saveWebhook({ ...w.destination, enabled: !w.destination.enabled }, null);
      await load();
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      busy = null;
    }
  }

  async function remove(w: WebhookInfo) {
    const ok = await confirm(t('webhooks.confirmDelete', { name: w.destination.name }), {
      kind: 'warning',
    });
    if (!ok) return;
    busy = w.destination.id;
    try {
      await api.deleteWebhook(w.destination.id);
      say(true, t('webhooks.deleted'));
      await load();
    } catch (e) {
      say(false, humanizeError(e));
    } finally {
      busy = null;
    }
  }
</script>

<h1>{t('webhooks.title')}</h1>
<p class="desc dim">{t('webhooks.intro')}</p>
<p class="note"><Icon name="key" size={14} /> {t('webhooks.secretNote')}</p>

{#if loaded && list.length === 0 && !draft}
  <p class="desc dim">{t('webhooks.empty')}</p>
{/if}

{#each list as w (w.destination.id)}
  <div class="dest" class:off={!w.destination.enabled}>
    <div class="d-main">
      <div class="d-top">
        <span class="d-name">{w.destination.name}</span>
        <span class="tag">
          {w.destination.kind === 'discord' ? t('webhooks.kindDiscord') : t('webhooks.kindJson')}
        </span>
        <span class="url" class:missing={!w.has_url}>
          {#if w.has_url}<Icon name="check" size={12} /> {t('webhooks.urlSaved')}{:else}{t(
              'webhooks.urlMissing',
            )}{/if}
        </span>
      </div>
      <div class="d-events dim">
        {w.destination.events.map((e) => t(`webhooks.event.${e}`)).join(' · ')}
      </div>
    </div>
    <div class="d-acts">
      <label class="switch" title={t('webhooks.enabled')}>
        <input
          type="checkbox"
          checked={w.destination.enabled}
          disabled={busy === w.destination.id}
          onchange={() => toggleEnabled(w)}
        />
      </label>
      <button
        class="mini"
        disabled={!w.has_url || busy !== null}
        onclick={() => test(w.destination.kind, null, w.destination.id, `test-${w.destination.id}`)}
      >
        {#if busy === `test-${w.destination.id}`}<span class="spinner"></span>{:else}<Icon
            name="send"
            size={13}
          />{/if}
        {t('webhooks.test')}
      </button>
      <button class="mini" aria-label={t('automations.edit')} onclick={() => startEdit(w)}>
        <Icon name="pencil" size={13} />
      </button>
      <button class="mini danger" aria-label={t('webhooks.delete')} onclick={() => remove(w)}>
        <Icon name="trash" size={13} />
      </button>
    </div>
  </div>
{/each}

{#if draft}
  <div class="editor">
    <div class="ed-title">{draft.id ? t('webhooks.editDest') : t('webhooks.newDest')}</div>
    <label class="field">
      <span class="lab">{t('webhooks.name')}</span>
      <input class="txt" bind:value={draft.name} maxlength="60" placeholder={t('webhooks.namePlaceholder')} />
    </label>
    <div class="field">
      <span class="lab">{t('webhooks.kind')}</span>
      <div class="seg">
        <button class:on={draft.kind === 'discord'} onclick={() => (draft!.kind = 'discord')}>
          {t('webhooks.kindDiscord')}
        </button>
        <button class:on={draft.kind === 'json'} onclick={() => (draft!.kind = 'json')}>
          {t('webhooks.kindJson')}
        </button>
      </div>
    </div>
    <label class="field">
      <span class="lab">{t('webhooks.url')}</span>
      <!-- Champ masqué : l'URL est un secret ; jamais pré-remplie depuis le stockage. -->
      <input
        class="txt"
        type="password"
        autocomplete="off"
        spellcheck="false"
        bind:value={draft.url}
        placeholder={draft.hasUrl
          ? t('webhooks.urlKeep')
          : draft.kind === 'discord'
            ? t('webhooks.urlPlaceholderDiscord')
            : t('webhooks.urlPlaceholderJson')}
      />
      {#if draft.url.trim() !== '' && !urlLooksValid(draft.kind, draft.url)}
        <span class="err">{t('webhooks.urlInvalid')}</span>
      {/if}
    </label>
    <div class="field">
      <span class="lab">{t('webhooks.events')}</span>
      <div class="events">
        {#each EVENTS as e (e)}
          <label class="check">
            <input type="checkbox" checked={draft.events.includes(e)} onchange={() => toggleEvent(e)} />
            <span>{t(`webhooks.event.${e}`)}</span>
          </label>
        {/each}
      </div>
      {#if draft.events.length === 0}<span class="err">{t('webhooks.noEvents')}</span>{/if}
    </div>
    <label class="check">
      <input type="checkbox" bind:checked={draft.enabled} />
      <span>{t('webhooks.enabled')}</span>
    </label>
    <div class="ed-actions">
      <button
        class="btn btn--ghost"
        disabled={busy !== null ||
          (draft.url.trim() === '' ? !draft.hasUrl : !urlLooksValid(draft.kind, draft.url))}
        onclick={() =>
          test(draft!.kind, draft!.url.trim() || null, draft!.id || null, 'test-draft')}
      >
        {#if busy === 'test-draft'}<span class="spinner"></span>{:else}<Icon name="send" size={14} />{/if}
        {busy === 'test-draft' ? t('webhooks.testing') : t('webhooks.test')}
      </button>
      <span class="spacer"></span>
      <button class="btn btn--ghost" onclick={() => (draft = null)} disabled={busy === 'save'}>
        {t('webhooks.cancel')}
      </button>
      <button class="btn" onclick={save} disabled={!draftValid || busy !== null}>
        {#if busy === 'save'}<span class="spinner"></span>{/if}
        {busy === 'save' ? t('webhooks.saving') : t('webhooks.save')}
      </button>
    </div>
  </div>
{:else}
  <div class="actions">
    <button class="btn btn--ghost" onclick={startNew}>
      <Icon name="plus" size={14} />
      {t('webhooks.add')}
    </button>
  </div>
{/if}

{#if message}
  <p class="msg" class:ok={message.ok}>{message.text}</p>
{/if}

<style>
  h1 {
    margin: 0 0 22px;
    font-size: 22px;
    letter-spacing: -0.02em;
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
    margin: 0 0 18px;
    padding: 9px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-muted);
  }
  .dest {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .dest.off .d-main {
    opacity: 0.55;
  }
  .d-main {
    flex: 1;
    min-width: 0;
  }
  .d-top {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .d-name {
    font-weight: 800;
    font-size: 14px;
  }
  .tag {
    font-size: 10.5px;
    font-weight: 800;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 6px;
  }
  .url {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 11.5px;
    color: var(--state-running);
  }
  .url.missing {
    color: var(--state-pending);
  }
  .d-events {
    font-size: 12px;
    margin-top: 3px;
  }
  .d-acts {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .switch {
    display: inline-flex;
    padding: 0 4px;
    cursor: pointer;
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
    padding: 5px 10px;
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
    margin-top: 16px;
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
  .events {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 6px 16px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
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
    margin-top: 16px;
  }
  .spacer {
    flex: 1;
  }
  .msg {
    font-size: 13px;
    margin: 12px 0 0;
    color: var(--state-danger);
  }
  .msg.ok {
    color: var(--state-running);
  }
</style>
