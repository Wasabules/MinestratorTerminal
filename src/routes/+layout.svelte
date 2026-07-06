<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { authStore, setAuth } from '$lib/stores/auth.svelte';
  import { api } from '$lib/ipc';
  import { applyInitialTheme } from '$lib/theme';
  import { initLocale, t } from '$lib/i18n';
  import { initColors } from '$lib/servers/colors.svelte';
  import { initSftpView } from '$lib/sftp/columns.svelte';
  import UpdateBanner from '$lib/components/UpdateBanner.svelte';

  let { children } = $props();

  // Vue narrowée de l'état d'auth (ergonomie du typage).
  const auth = $derived(authStore.value);

  onMount(async () => {
    applyInitialTheme();
    initLocale();
    initColors();
    initSftpView();
    try {
      if (!(await api.hasStoredKey())) {
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
    const path = $page.url.pathname;
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

<style>
  .boot {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    font-size: 15px;
  }
</style>
