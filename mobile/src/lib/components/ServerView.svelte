<script lang="ts">
  import { t } from "../i18n";
  import type { ServerListItem } from "../types";
  import BottomNav from "./BottomNav.svelte";
  import OverviewView from "./views/OverviewView.svelte";
  import ConsoleView from "./views/ConsoleView.svelte";
  import PlayersView from "./views/PlayersView.svelte";

  let { server, onBack }: { server: ServerListItem; onBack: () => void } = $props();

  let active = $state("overview");

  const tabs = [
    { id: "overview", label: t("nav.overview"), icon: "📊" },
    { id: "console", label: t("nav.console"), icon: "🖥️" },
    { id: "players", label: t("nav.players"), icon: "👥" },
  ];
</script>

<div class="shell">
  <header class="bar">
    <button class="back" onclick={onBack} aria-label="Retour">‹</button>
    <span class="title selectable">{server.name}</span>
  </header>

  <main>
    {#if active === "overview"}
      <OverviewView serverId={server.id} />
    {:else if active === "console"}
      <ConsoleView serverId={server.id} />
    {:else if active === "players"}
      <PlayersView serverId={server.id} />
    {/if}
  </main>

  <BottomNav {tabs} {active} onSelect={(id) => (active = id)} />
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100dvh;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: calc(var(--safe-top) + 8px) 12px 8px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .back {
    background: transparent;
    border: none;
    color: var(--text);
    font-size: 28px;
    line-height: 1;
    width: 40px;
    height: 40px;
  }
  .title {
    font-size: 17px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  main {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    /* laisse la place à la BottomNav fixe */
    padding-bottom: calc(var(--nav-height) + var(--safe-bottom));
  }
</style>
