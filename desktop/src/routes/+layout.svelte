<script lang="ts">
  // Polices embarquées (paquets @fontsource, licence OFL) : servies par l'app, jamais par un
  // CDN — le webview ne fait aucune requête réseau.
  import '@fontsource-variable/nunito';
  import '@fontsource-variable/jetbrains-mono';
  import '../app.css';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { authStore, setAuth } from '#lib/stores/auth.svelte.ts';
  import { api } from '#lib/ipc.ts';
  import { applyInitialTheme } from '#lib/theme.ts';
  import { initLocale, t } from '#lib/i18n/index.ts';
  import { initColors } from '#lib/servers/colors.svelte.ts';
  import { initSftpView } from '#lib/sftp/columns.svelte.ts';
  import { initTabMode } from '#lib/tabs/mode.svelte.ts';
  import { initCloseBehavior } from '#lib/close.svelte.ts';
  import { initDebug } from '#lib/debug.svelte.ts';
  import { initFake, isFake } from '#lib/fake.svelte.ts';
  import UpdateBanner from '#lib/components/UpdateBanner.svelte';
  import CloseDialog from '#lib/components/CloseDialog.svelte';

  let { children } = $props();

  // Vue narrowée de l'état d'auth (ergonomie du typage).
  const auth = $derived(authStore.value);

  onMount(async () => {
    applyInitialTheme();
    initLocale();
    initColors();
    initSftpView();
    initTabMode();
    initCloseBehavior();
    initDebug();
    initFake();
    try {
      if (!await api.hasStoredKey()) {
        setAuth({ status: 'signed_out' });
        return;
      }
      // Une clé existe : on la valide en récupérant le profil.
      const user = await api.getUser();
      setAuth({ status: 'signed_in', user });
    } catch {
      // Clé présente mais invalide / injoignable → on repart de l'onboarding.
      setAuth({ status: 'signed_out' });
    }
  });

  // Garde de navigation centralisée.
  $effect(() => {
    const path = page.url.pathname;
    if (auth.status === 'signed_out' && path !== '/onboarding') {
      goto('/onboarding');
    } else if (auth.status === 'signed_in' && path === '/onboarding') {
      goto('/');
    }
  });
</script>

{#if auth.status === 'loading'}
  <div class="boot">
    <span class="spinner"></span>
    <span class="muted">{t('common.connecting')}</span>
  </div>
{:else}
  {@render children()}
{/if}

<UpdateBanner />
<CloseDialog />

{#if isFake()}
  <!-- Repère permanent : sans lui, on peut croire lire de vraies données, ou publier une capture
       en pensant le mode actif alors qu'il ne l'est pas. -->
  <div class="fakebar">{t('settings.fakeBanner')}</div>
{/if}

<style>
  .fakebar {
    position: fixed;
    z-index: 999;
    left: 50%;
    bottom: 10px;
    transform: translateX(-50%);
    padding: 4px 12px;
    border-radius: 999px;
    /* Violet « veille » de l'End, identique dans les deux thèmes (encre sombre : 8:1). */
    background: #c79bff;
    color: #1e0b3a;
    font-size: 11px;
    font-weight: 800;
    letter-spacing: 0.03em;
    pointer-events: none;
    opacity: 0.85;
  }
  .boot {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    font-size: 15px;
  }
</style>
