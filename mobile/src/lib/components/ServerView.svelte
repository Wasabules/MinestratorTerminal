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
  import PropertiesView from "./views/PropertiesView.svelte";
  import MarketView from "./views/MarketView.svelte";
  import SchedulesView from "./views/SchedulesView.svelte";
  import Icon from "./Icon.svelte";
  import { portal } from "../portal";

  let {
    server,
    mybox,
    onBack,
  }: { server: ServerListItem; mybox: MyBoxSummary | null; onBack: () => void } = $props();

  interface TabDef {
    id: string;
    label: string;
    icon: string;
  }

  /**
   * Vues du serveur, dans l'ordre du pager. Les capacités du jeu (`GameCapabilities`, source
   * unique côté core) décident de ce qui existe : Joueurs pour les jeux qui en exposent,
   * Paramètres là où l'hôte publie `server.properties`, Mods pour la marketplace Minecraft.
   */
  const caps = $derived(server.capabilities);
  const allTabs = $derived<TabDef[]>([
    { id: "overview", label: t("nav.overview"), icon: "overview" },
    { id: "console", label: t("nav.console"), icon: "console" },
    ...(caps?.players === false ? [] : [{ id: "players", label: t("nav.players"), icon: "players" }]),
    { id: "files", label: t("nav.files"), icon: "files" },
    { id: "backups", label: t("nav.backups"), icon: "archive" },
    ...(caps?.properties ? [{ id: "properties", label: t("nav.properties"), icon: "sliders" }] : []),
    ...(caps?.mods === "minecraft" ? [{ id: "market", label: t("nav.market"), icon: "package" }] : []),
    // Tâches planifiées du panel MineStrator : tous les jeux en ont.
    { id: "schedules", label: t("nav.schedules"), icon: "clock" },
  ]);

  /**
   * Cinq emplacements au plus dans la barre du bas : au-delà, les libellés ne tiennent plus sur
   * un téléphone. Les vues en trop passent derrière « Plus » (feuille) ; le pager et le balayage
   * les gardent toutes, dans l'ordre.
   */
  const MAX_SLOTS = 5;
  const primary = $derived(allTabs.length <= MAX_SLOTS ? allTabs : allTabs.slice(0, MAX_SLOTS - 1));
  const overflow = $derived(allTabs.length <= MAX_SLOTS ? [] : allTabs.slice(MAX_SLOTS - 1));
  const order = $derived(allTabs.map((x) => x.id));

  /** Onglet courant, mémorisé pour survivre à un rechargement du webview (cf. `+page.svelte`). */
  const LAST_TAB = "mnstr-last-tab";
  let active = $state(readLastTab());
  /** Onglet mémorisé absent pour ce jeu (ex. Paramètres sur un serveur non Minecraft) : Aperçu. */
  const current = $derived(order.includes(active) ? active : "overview");
  const overflowActive = $derived(overflow.find((x) => x.id === current) ?? null);
  let moreOpen = $state(false);

  /** Barre du bas : l'emplacement « Plus » prend le nom de la vue ouverte derrière lui. */
  const navTabs = $derived<TabDef[]>(
    overflow.length === 0
      ? primary
      : [
          ...primary,
          overflowActive
            ? { id: "more", label: overflowActive.label, icon: overflowActive.icon }
            : { id: "more", label: t("nav.more"), icon: "more" },
        ],
  );

  function readLastTab(): string {
    try {
      return localStorage.getItem(LAST_TAB) ?? "overview";
    } catch {
      return "overview";
    }
  }
  let navHidden = $state(false);
  const idx = $derived(Math.max(0, order.indexOf(current)));

  function onNavSelect(id: string) {
    if (id === "more") moreOpen = true;
    else goto(id);
  }

  function goto(id: string) {
    moreOpen = false;
    try {
      localStorage.setItem(LAST_TAB, id);
    } catch {
      /* stockage indisponible : sans conséquence */
    }
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
    blocked = !!target.closest(
      ".cm-editor, input, textarea, select, .suggest, .editor, .sheet, .dialog, [data-noswipe]",
    );
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
    <GameIcon src={server.egg_icon_url} name={server.egg_name} size={26} />
    <span class="title selectable">{server.name}</span>
    {#if server.support_ref}
      <!-- Référence à donner au support : utile mais secondaire, d'où le coin discret. -->
      <span class="ref selectable" title={t("overview.supportRef")}>{server.support_ref}</span>
    {/if}
  </header>

  <div
    class="pages"
    class:nav-hidden={navHidden}
    role="group"
    ontouchstart={onTouchStart}
    ontouchend={onTouchEnd}
  >
    <div class="track" style="transform: translateX(-{idx * 100}%)">
      {#each allTabs as tab (tab.id)}
        <section class="page">
          {#if tab.id === "overview"}
            <OverviewView {server} {mybox} active={current === "overview"} onNavigate={goto} />
          {:else if tab.id === "console"}
            <ConsoleView
              serverId={server.id}
              active={current === "console"}
              onFocusChange={(f) => (navHidden = f)}
            />
          {:else if tab.id === "players"}
            <PlayersView serverId={server.id} active={current === "players"} />
          {:else if tab.id === "files"}
            <SftpView serverId={server.id} active={current === "files"} />
          {:else if tab.id === "backups"}
            <BackupsView
              serverId={server.id}
              active={current === "backups"}
              lastBackupUrl={server.last_backup_url}
            />
          {:else if tab.id === "properties"}
            <PropertiesView {server} active={current === "properties"} />
          {:else if tab.id === "market"}
            <MarketView serverId={server.id} active={current === "market"} />
          {:else if tab.id === "schedules"}
            <SchedulesView serverId={server.id} active={current === "schedules"} />
          {/if}
        </section>
      {/each}
    </div>
  </div>

  {#if !navHidden}
    <BottomNav tabs={navTabs} active={overflowActive ? "more" : current} onSelect={onNavSelect} />
  {/if}
</div>

{#if moreOpen}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => (moreOpen = false)}
    onkeydown={(e) => e.key === "Escape" && (moreOpen = false)}
  ></div>
  <div class="sheet" use:portal role="menu" aria-label={t("nav.moreTitle")}>
    <div class="sheet-title">{t("nav.moreTitle")}</div>
    {#each overflow as tab (tab.id)}
      <button role="menuitem" class:on={tab.id === current} onclick={() => goto(tab.id)}>
        <Icon name={tab.icon} size={20} />
        <span>{tab.label}</span>
        {#if tab.id === current}<span class="here"><Icon name="check" size={18} /></span>{/if}
      </button>
    {/each}
  </div>
{/if}

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
  .ref {
    flex: none;
    margin-left: 8px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--text) 5%, transparent);
    color: var(--text-dim);
    font-size: 11px;
    letter-spacing: 0.04em;
    font-variant-numeric: tabular-nums;
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

  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 40;
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 10px calc(var(--safe-bottom) + 14px);
    background: var(--elevated);
    border-top-left-radius: 16px;
    border-top-right-radius: 16px;
  }
  .sheet-title {
    padding: 8px 12px 10px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .sheet button {
    display: flex;
    align-items: center;
    gap: 14px;
    min-height: 52px;
    padding: 0 12px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text);
    font-size: 16px;
    text-align: left;
  }
  .sheet button.on {
    background: color-mix(in srgb, var(--brand-primary) 12%, transparent);
    color: var(--brand-primary);
  }
  .here {
    display: inline-flex;
    margin-left: auto;
  }
</style>
