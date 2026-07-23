<script lang="ts">
  import { onMount } from 'svelte';
  import { authStore } from '$lib/stores/auth.svelte';
  import { tabs } from '$lib/tabs/tabs.svelte';
  import { readDetachSpec } from '$lib/windows';
  import Workspace from '$lib/components/Workspace.svelte';

  const auth = $derived(authStore.value);

  // Fenêtre issue d'un détachement : ouvre l'onglet demandé.
  onMount(() => {
    const spec = readDetachSpec();
    if (spec) tabs.openNew(spec.serverId, spec.serverName, spec.view);
  });
</script>

{#if auth.status === 'signed_in'}
  <Workspace />
{:else}
  <div class="boot"><span class="spinner"></span></div>
{/if}

<style>
  .boot {
    height: 100vh;
    display: grid;
    place-items: center;
  }
</style>
