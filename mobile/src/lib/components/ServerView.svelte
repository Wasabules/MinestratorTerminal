<script lang="ts">
  import { t } from "../i18n";
  import type { ServerListItem } from "../types";
  import BottomNav from "./BottomNav.svelte";
  import OverviewView from "./views/OverviewView.svelte";
  import ConsoleView from "./views/ConsoleView.svelte";
  import PlayersView from "./views/PlayersView.svelte";
  import SftpView from "./views/SftpView.svelte";

  let { server, onBack }: { server: ServerListItem; onBack: () => void } = $props();

  let active = $state("overview");
  // Masque la barre du bas quand la console a le focus → l'input se cale au-dessus du clavier.
  let navHidden = $state(false);

  const tabs = [
    { id: "overview", label: t("nav.overview"), icon: "overview" },
    { id: "console", label: t("nav.console"), icon: "console" },
    { id: "players", label: t("nav.players"), icon: "players" },
    { id: "files", label: t("nav.files"), icon: "files" },
  ];

  function select(id: string) {
    if (id !== "console") navHidden = false;
    active = id;
  }
</script>

<div class="shell">
  <header class="bar">
    <button class="back" onclick={onBack} aria-label="Retour">‹</button>
    <span class="title selectable">{server.name}</span>
  </header>

  <main class:nav-hidden={navHidden}>
    {#if active === "overview"}
      <OverviewView serverId={server.id} />
    {:else if active === "console"}
      <ConsoleView serverId={server.id} onFocusChange={(f) => (navHidden = f)} />
    {:else if active === "players"}
      <PlayersView serverId={server.id} />
    {:else if active === "files"}
      <SftpView serverId={server.id} />
    {/if}
  </main>

  {#if !navHidden}
    <BottomNav {tabs} {active} onSelect={select} />
  {/if}
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
    padding-bottom: calc(var(--nav-height) + var(--safe-bottom));
  }
  main.nav-hidden {
    padding-bottom: 0;
  }
</style>
