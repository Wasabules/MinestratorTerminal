<script lang="ts">
  import { t } from "../i18n";
  import type { MyBoxSummary, ServerListItem } from "../types";
  import BottomNav from "./BottomNav.svelte";
  import GameIcon from "./GameIcon.svelte";
  import OverviewView from "./views/OverviewView.svelte";
  import ConsoleView from "./views/ConsoleView.svelte";
  import PlayersView from "./views/PlayersView.svelte";
  import SftpView from "./views/SftpView.svelte";
  import BackupsView from "./views/BackupsView.svelte";

  let {
    server,
    mybox,
    onBack,
  }: { server: ServerListItem; mybox: MyBoxSummary | null; onBack: () => void } = $props();

  const order = ["overview", "console", "players", "files", "backups"];
  let active = $state("overview");
  let navHidden = $state(false);
  const idx = $derived(order.indexOf(active));

  const tabs = [
    { id: "overview", label: t("nav.overview"), icon: "overview" },
    { id: "console", label: t("nav.console"), icon: "console" },
    { id: "players", label: t("nav.players"), icon: "players" },
    { id: "files", label: t("nav.files"), icon: "files" },
    { id: "backups", label: t("nav.backups"), icon: "archive" },
  ];

  function goto(id: string) {
    if (id !== "console") navHidden = false;
    active = id;
  }

  // --- Swipe horizontal pour changer d'onglet ---
  let startX = 0;
  let startY = 0;
  let blocked = false;

  function onTouchStart(e: TouchEvent) {
    const target = e.target as HTMLElement;
    // Ne pas capturer le swipe sur zones interactives/scroll horizontal.
    blocked = !!target.closest(".cm-editor, input, textarea, .suggest, .editor, .sheet, .dialog");
    startX = e.touches[0].clientX;
    startY = e.touches[0].clientY;
  }
  function onTouchEnd(e: TouchEvent) {
    if (blocked) return;
    const dx = e.changedTouches[0].clientX - startX;
    const dy = e.changedTouches[0].clientY - startY;
    if (Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(dy) * 1.5) {
      if (dx < 0 && idx < order.length - 1) goto(order[idx + 1]);
      else if (dx > 0 && idx > 0) goto(order[idx - 1]);
    }
  }
</script>

<div class="shell">
  <header class="bar">
    <button class="back" onclick={onBack} aria-label="Retour">‹</button>
    <GameIcon src={server.egg_icon} size={26} />
    <span class="title selectable">{server.name}</span>
  </header>

  <div
    class="pages"
    class:nav-hidden={navHidden}
    role="group"
    ontouchstart={onTouchStart}
    ontouchend={onTouchEnd}
  >
    <div class="track" style="transform: translateX(-{idx * 100}%)">
      <section class="page">
        <OverviewView {server} {mybox} active={active === "overview"} />
      </section>
      <section class="page">
        <ConsoleView
          serverId={server.id}
          active={active === "console"}
          onFocusChange={(f) => (navHidden = f)}
        />
      </section>
      <section class="page">
        <PlayersView serverId={server.id} active={active === "players"} />
      </section>
      <section class="page">
        <SftpView serverId={server.id} active={active === "files"} />
      </section>
      <section class="page">
        <BackupsView serverId={server.id} active={active === "backups"} />
      </section>
    </div>
  </div>

  {#if !navHidden}
    <BottomNav {tabs} {active} onSelect={goto} />
  {/if}
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100dvh;
    overflow: hidden;
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
  .pages {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    position: relative;
  }
  .track {
    display: flex;
    height: 100%;
    width: 100%;
    transition: transform 0.25s ease;
  }
  .page {
    flex: 0 0 100%;
    width: 100%;
    height: 100%;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    padding-bottom: calc(var(--nav-height) + var(--safe-bottom));
  }
  .pages.nav-hidden .page {
    padding-bottom: 0;
  }
</style>
