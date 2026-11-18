<script lang="ts">
  import { onMount } from 'svelte';
  import { authStore } from '#lib/stores/auth.svelte.ts';
  import { tabs } from '#lib/tabs/tabs.svelte.ts';
  import { readDetachSpec } from '#lib/windows.ts';
  import Workspace from '#lib/components/Workspace.svelte';

  const auth = $derived(authStore.value);

  // Fenêtre issue d'un détachement : ouvre l'onglet demandé.
  onMount(() => {
    const spec = readDetachSpec();
    if (spec) tabs.openNew(spec.serverId, spec.serverName, spec.view);
  });
</script>

{#if auth.status === 'signed_in'}
  <!-- Changement de compte : le workspace est remonté, toutes ses listes se rechargent. -->
  {#key auth.user.id}
    <Workspace />
  {/key}
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
