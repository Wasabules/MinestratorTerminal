<script lang="ts">
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { authStore } from '#lib/stores/auth.svelte.ts';
  import {
    accountsStore,
    applyAccountSwitch,
    openAddAccount,
    refreshAccounts,
  } from '#lib/stores/accounts.svelte.ts';
  import { openSettingsSection } from '#lib/settings/nav.svelte.ts';
  import { tabs } from '#lib/tabs/tabs.svelte.ts';
  import { toggleTheme } from '#lib/theme.ts';
  import { t, getLocale, setLocale, LOCALES, type Locale } from '#lib/i18n/index.ts';
  import type { Account } from '#lib/types.ts';
  import Icon from './Icon.svelte';
  import AddAccountDialog from './AddAccountDialog.svelte';

  let open = $state(false);
  let switching = $state<number | null>(null);
  let error = $state<string | null>(null);
  const auth = $derived(authStore.value);
  const pseudo = $derived(auth.status === 'signed_in' ? auth.user.pseudo : '');
  const mail = $derived(auth.status === 'signed_in' ? auth.user.mail : '');
  /** Autres comptes enregistrés, proposés à la bascule. */
  const others = $derived(accountsStore.list.filter((a) => !a.active));

  function chooseLocale(code: Locale) {
    setLocale(code);
  }

  function toggle() {
    open = !open;
    error = null;
    if (open) void refreshAccounts();
  }

  async function switchTo(a: Account) {
    switching = a.id;
    error = null;
    try {
      const current = await api.accountSwitch(a.id);
      open = false;
      await applyAccountSwitch(current);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      switching = null;
    }
  }

  function addAccount() {
    open = false;
    openAddAccount();
  }

  function manageAccounts() {
    open = false;
    openSettingsSection('accounts');
  }

  /** Retire le compte actif : bascule sur un autre compte s'il en reste, sinon écran de connexion. */
  async function logout() {
    open = false;
    try {
      await applyAccountSwitch(await api.logout());
    } catch (e) {
      error = humanizeError(e);
      open = true;
    }
  }

  onMount(() => void refreshAccounts());
</script>

<div class="usermenu">
  <button class="trigger" onclick={toggle} title={pseudo}>
    <span class="avatar" aria-hidden="true">{pseudo.charAt(0).toUpperCase()}</span>
    <span class="pseudo">{pseudo}</span>
    <span class="caret" aria-hidden="true">▾</span>
  </button>

  {#if open}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (open = false)}></button>
    <div class="menu" role="menu">
      <div class="head">
        <span class="h-pseudo">{pseudo}</span>
        <span class="h-mail">{mail}</span>
      </div>

      <div class="sep"></div>

      <!-- Comptes : bascule vers un autre compte enregistré, ajout, gestion. -->
      {#if others.length > 0}
        <div class="group-label">{t('account.switchTo')}</div>
        {#each others as a (a.id)}
          <button
            class="item"
            role="menuitem"
            onclick={() => switchTo(a)}
            disabled={switching !== null}
          >
            <span class="mini-avatar" aria-hidden="true">{a.pseudo.charAt(0).toUpperCase()}</span>
            <span class="acc-name">{a.pseudo}</span>
            {#if switching === a.id}<span class="spinner small"></span>{/if}
          </button>
        {/each}
      {/if}
      <button class="item" role="menuitem" onclick={addAccount}>
        <span class="i-ico"><Icon name="plus" size={15} /></span>{t('account.add')}
      </button>
      {#if accountsStore.list.length > 1}
        <button class="item" role="menuitem" onclick={manageAccounts}>
          <span class="i-ico"><Icon name="users" size={15} /></span>{t('account.manage')}
        </button>
      {/if}
      {#if error}<p class="err" role="alert">{error}</p>{/if}

      <div class="sep"></div>

      <button
        class="item"
        role="menuitem"
        onclick={() => {
          open = false;
          tabs.openNotifications();
        }}
      >
        <span class="i-ico"><Icon name="bell" size={15} /></span>{t('notifications.title')}
      </button>

      <button
        class="item"
        role="menuitem"
        onclick={() => {
          open = false;
          tabs.openSettings();
        }}
      >
        <span class="i-ico"><Icon name="settings" size={15} /></span>{t('settings.title')}
      </button>

      <button class="item" role="menuitem" onclick={() => toggleTheme()}>
        <span class="i-ico"><Icon name="contrast" size={15} /></span>{t('common.theme')}
      </button>

      <div class="group-label">{t('common.language')}</div>
      <div class="langs">
        {#each LOCALES as loc (loc.code)}
          <button
            class="lang"
            class:active={getLocale() === loc.code}
            onclick={() => chooseLocale(loc.code)}
          >
            {loc.label}
          </button>
        {/each}
      </div>

      <div class="sep"></div>

      <button class="item danger" role="menuitem" onclick={logout}>
        <span class="i-ico"><Icon name="power" size={15} /></span>{t('common.logout')}
      </button>
    </div>
  {/if}
</div>

<!-- Seul point de rendu de « Ajouter un compte » (ouvert via `openAddAccount()`). -->
{#if accountsStore.adding}
  <AddAccountDialog onclose={() => (accountsStore.adding = false)} />
{/if}

<style>
  .usermenu {
    position: relative;
    flex: none;
  }
  .trigger {
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius);
    cursor: pointer;
    font: inherit;
    color: var(--text-muted);
    padding: 5px 9px 5px 6px;
  }
  .trigger:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .avatar {
    width: 24px;
    height: 24px;
    border-radius: 7px;
    display: grid;
    place-items: center;
    font-size: 12px;
    font-weight: 700;
    color: #fff;
    background: var(--brand-gradient);
  }
  .pseudo {
    font-size: 13px;
    font-weight: 600;
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .caret {
    font-size: 10px;
    color: var(--text-dim);
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 30;
    cursor: default;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 31;
    min-width: 220px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .head {
    display: flex;
    flex-direction: column;
    padding: 6px 8px 8px;
  }
  .h-pseudo {
    font-weight: 700;
  }
  .h-mail {
    font-size: 12px;
    color: var(--text-dim);
  }
  .sep {
    height: 1px;
    background: var(--border);
    margin: 5px 0;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    padding: 9px 10px;
    border-radius: 8px;
  }
  .item:hover {
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .item.danger {
    color: var(--state-danger);
  }
  .item:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .mini-avatar {
    flex: none;
    width: 18px;
    height: 18px;
    border-radius: 5px;
    display: grid;
    place-items: center;
    font-size: 10px;
    font-weight: 700;
    color: #fff;
    background: var(--brand-gradient);
    opacity: 0.8;
  }
  .acc-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spinner.small {
    width: 12px;
    height: 12px;
  }
  .err {
    margin: 4px 10px;
    font-size: 12px;
    color: var(--state-danger);
  }
  .i-ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    opacity: 0.85;
  }
  .group-label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-dim);
    padding: 8px 10px 4px;
  }
  .langs {
    display: flex;
    gap: 6px;
    padding: 0 8px 4px;
  }
  .lang {
    flex: 1;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-muted);
    padding: 7px 8px;
  }
  .lang.active {
    color: #fff;
    background: var(--brand-primary);
    border-color: transparent;
  }
</style>
