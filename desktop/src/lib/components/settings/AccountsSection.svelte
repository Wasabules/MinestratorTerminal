<script lang="ts">
  /**
   * Réglages « Comptes » : comptes MineStrator enregistrés sur l'appareil, bascule, ajout, retrait.
   * Les clés restent dans le stockage sécurisé (compte actif sous `api-key`, les autres à l'écart) ;
   * seule la liste non secrète transite ici. Le compte surveillé par le service d'arrière-plan est
   * signalé : le service le garde quel que soit le compte actif de l'app.
   */
  import { onMount } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t, getLocale } from '#lib/i18n/index.ts';
  import {
    accountsStore,
    applyAccountSwitch,
    openAddAccount,
    refreshAccounts,
  } from '#lib/stores/accounts.svelte.ts';
  import type { Account } from '#lib/types.ts';
  import { avatarStyle } from '#lib/servers/colors.svelte.ts';

  let busy = $state<number | null>(null);
  let message = $state<{ ok: boolean; text: string } | null>(null);
  /** Compte surveillé par le service d'arrière-plan (`null` : aucun ou service absent). */
  let serviceAccount = $state<number | null>(null);

  const accounts = $derived(accountsStore.list);

  function addedOn(a: Account): string {
    try {
      return new Date(a.added_at * 1000).toLocaleDateString(getLocale());
    } catch {
      return '';
    }
  }

  async function switchTo(a: Account) {
    busy = a.id;
    message = null;
    try {
      const current = await api.accountSwitch(a.id);
      await applyAccountSwitch(current);
    } catch (e) {
      message = { ok: false, text: humanizeError(e) };
    } finally {
      busy = null;
    }
  }

  async function remove(a: Account) {
    let text = t(a.active ? 'account.confirmRemoveActive' : 'account.confirmRemove', {
      pseudo: a.pseudo,
    });
    if (serviceAccount === a.id) text += `\n\n${t('account.confirmRemoveService')}`;
    const ok = await confirm(text, { kind: 'warning' });
    if (!ok) return;
    busy = a.id;
    message = null;
    try {
      const current = await api.accountRemove(a.id);
      if (a.active) {
        await applyAccountSwitch(current);
      } else {
        await refreshAccounts();
        message = { ok: true, text: t('account.removed', { pseudo: a.pseudo }) };
      }
    } catch (e) {
      message = { ok: false, text: humanizeError(e) };
    } finally {
      busy = null;
    }
  }

  onMount(async () => {
    void refreshAccounts();
    try {
      const o = await api.serviceOverview();
      serviceAccount = o.status?.has_key ? (o.status.account_id ?? null) : null;
    } catch {
      serviceAccount = null;
    }
  });
</script>

<h1>{t('account.title')}</h1>
<p class="desc dim">{t('account.intro')}</p>

<ul class="list">
  {#each accounts as a (a.id)}
    <li class="acc" class:active={a.active}>
      <span class="avatar" style={avatarStyle(a.pseudo)} aria-hidden="true">{a.pseudo.charAt(0).toUpperCase()}</span>
      <div class="who">
        <div class="name">
          {a.pseudo}
          {#if a.active}<span class="badge on">{t('account.active')}</span>{/if}
          {#if serviceAccount === a.id}<span class="badge">{t('account.watchedByService')}</span>{/if}
        </div>
        <div class="meta dim">#{a.id} · {t('account.addedOn', { date: addedOn(a) })}</div>
      </div>
      <div class="acts">
        {#if !a.active}
          <button class="btn btn--ghost" onclick={() => switchTo(a)} disabled={busy !== null}>
            {busy === a.id ? t('account.switching') : t('account.switch')}
          </button>
        {/if}
        <button class="btn btn--ghost danger" onclick={() => remove(a)} disabled={busy !== null}>
          {t('account.remove')}
        </button>
      </div>
    </li>
  {:else}
    <li class="empty dim">{t('common.loading')}</li>
  {/each}
</ul>

<div class="actions">
  <button class="btn" onclick={openAddAccount} disabled={busy !== null}>
    {t('account.add')}
  </button>
</div>
{#if message}
  <p class="msg" class:ok={message.ok}>{message.text}</p>
{/if}

<p class="note">{t('account.activeUsage')}</p>
<p class="note">{t('account.serviceNote')}</p>

<style>
  h1 {
    margin: 0 0 22px;
    font-size: 22px;
    letter-spacing: -0.02em;
  }
  .desc {
    font-size: 13px;
    margin: 0 0 10px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .acc {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .avatar {
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    font-size: 14px;
    font-weight: 900;
    opacity: 0.75;
  }
  .acc.active .avatar {
    opacity: 1;
  }
  .who {
    flex: 1;
    min-width: 0;
  }
  .name {
    font-size: 14px;
    font-weight: 800;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    overflow-wrap: anywhere;
  }
  .meta {
    font-size: 12px;
    margin-top: 2px;
  }
  .badge {
    font-size: 11px;
    font-weight: 800;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-muted);
  }
  .badge.on {
    color: var(--on-accent);
    border-color: transparent;
    background: var(--accent);
  }
  .acts {
    flex: none;
    display: flex;
    gap: 8px;
  }
  .empty {
    padding: 12px 0;
    font-size: 13px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }
  .danger {
    color: var(--state-danger);
  }
  .msg {
    font-size: 13px;
    margin: 10px 0 0;
    color: var(--state-danger);
  }
  .msg.ok {
    color: var(--state-running);
  }
  .note {
    font-size: 13px;
    line-height: 1.55;
    margin: 14px 0 0;
    padding: 11px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-muted);
  }
</style>
