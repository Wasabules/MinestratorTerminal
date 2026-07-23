<script lang="ts">
  import { onMount } from 'svelte';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import { serverCaps, ensureCaps } from '$lib/games/capabilities.svelte';
  import ServerOverview from './views/ServerOverview.svelte';
  import ConsoleView from './views/ConsoleView.svelte';
  import SftpView from './views/SftpView.svelte';
  import PlayersView from './views/PlayersView.svelte';
  import MarketplaceView from './views/MarketplaceView.svelte';
  import SatisfactoryModsView from './views/SatisfactoryModsView.svelte';
  import ModMarketplaceView from './views/ModMarketplaceView.svelte';
  import AssistantView from './views/AssistantView.svelte';
  import BackupsView from './views/BackupsView.svelte';
  import ComingSoon from './views/ComingSoon.svelte';

  let { tab }: { tab: ServerTab } = $props();

  // Capacités du jeu de ce serveur : pilote quelles vues sont réellement rendues.
  const caps = $derived(serverCaps(tab.serverId));
  onMount(() => void ensureCaps(tab.serverId));
</script>

{#if tab.view === 'overview'}
  <ServerOverview {tab} />
{:else if tab.view === 'console'}
  <ConsoleView {tab} />
{:else if tab.view === 'sftp'}
  <SftpView {tab} />
{:else if tab.view === 'players'}
  {#if caps === undefined || caps.players}
    <PlayersView {tab} />
  {:else}
    <ComingSoon {tab} />
  {/if}
{:else if tab.view === 'mods'}
  {#if caps?.mods === 'ficsit'}
    <SatisfactoryModsView {tab} />
  {:else if caps?.mods === 'thunderstore' || caps?.mods === 'factorio' || caps?.mods === 'umod'}
    <ModMarketplaceView {tab} />
  {:else if caps?.mods === 'none'}
    <ComingSoon {tab} />
  {:else}
    <MarketplaceView {tab} />
  {/if}
{:else if tab.view === 'assistant'}
  <AssistantView {tab} />
{:else if tab.view === 'backups'}
  <BackupsView {tab} />
{:else}
  <ComingSoon {tab} />
{/if}
