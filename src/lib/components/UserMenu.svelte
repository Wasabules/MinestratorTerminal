<script lang="ts">
  import { api } from '$lib/ipc';
  import { authStore, setAuth } from '$lib/stores/auth.svelte';
  import { tabs } from '$lib/tabs/tabs.svelte';
  import { toggleTheme } from '$lib/theme';
  import { t, getLocale, setLocale, LOCALES, type Locale } from '$lib/i18n';
  import Icon from './Icon.svelte';

  let open = $state(false);
  const auth = $derived(authStore.value);
  const pseudo = $derived(auth.status === 'signed_in' ? auth.user.pseudo : '');
  const mail = $derived(auth.status === 'signed_in' ? auth.user.mail : '');

  function chooseLocale(code: Locale) {
    setLocale(code);
  }

  async function logout() {
    open = false;
    await api.logout();
    tabs.reset();
    setAuth({ status: 'signed_out' });
  }
</script>

<div class="usermenu">
  <button class="trigger" onclick={() => (open = !open)} title={pseudo}>
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
