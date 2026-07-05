<script lang="ts">
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import ServerOverview from './views/ServerOverview.svelte';
  import ConsoleView from './views/ConsoleView.svelte';
  import SftpView from './views/SftpView.svelte';
  import PlayersView from './views/PlayersView.svelte';
  import MarketplaceView from './views/MarketplaceView.svelte';
  import AssistantView from './views/AssistantView.svelte';
  import BackupsView from './views/BackupsView.svelte';
  import ComingSoon from './views/ComingSoon.svelte';

  let { tab }: { tab: ServerTab } = $props();
</script>

{#if tab.view === 'overview'}
  <ServerOverview {tab} />
{:else if tab.view === 'console'}
  <ConsoleView {tab} />
{:else if tab.view === 'sftp'}
  <SftpView {tab} />
{:else if tab.view === 'players'}
  <PlayersView {tab} />
{:else if tab.view === 'mods'}
  <MarketplaceView {tab} />
{:else if tab.view === 'assistant'}
  <AssistantView {tab} />
{:else if tab.view === 'backups'}
  <BackupsView {tab} />
{:else}
  <ComingSoon {tab} />
{/if}
