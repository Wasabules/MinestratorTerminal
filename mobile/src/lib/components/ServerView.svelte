<script lang="ts">
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import { biomeVars, serverBiome } from "../servers/colors.svelte";
  import { runtimeMeta } from "../status";
  import type { ConsoleStats, LiveLight, MyBoxSummary, ServerListItem } from "../types";
  import BottomNav from "./BottomNav.svelte";
  import BiomeBlock from "./BiomeBlock.svelte";
  import BiomeLandscape from "./BiomeLandscape.svelte";
  import BiomePicker from "./BiomePicker.svelte";
  import AnonToggle from "./AnonToggle.svelte";
  import PowerControl from "./PowerControl.svelte";
  import ServerHero from "./ServerHero.svelte";
  import StatusDot from "./StatusDot.svelte";
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

  /** Biome du serveur : il teinte toute la vue (classe `.biome`) et dessine l'en-tête. */
  const biome = $derived(serverBiome(server.id));
  let pickerOpen = $state(false);

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

  // --- État d'exécution, partagé par l'en-tête (pastille, alimentation) et l'Aperçu ---

  let live = $state<LiveLight | null>(null);
  let stats = $state<ConsoleStats | null>(null);
  let liveError = $state<string | null>(null);
  let lastStats = 0;

  async function refreshLive() {
    try {
      live = await api.liveLight(server.id);
      liveError = null;
    } catch (err) {
      liveError = humanizeError(err);
    }
  }
  async function refreshStats() {
    lastStats = Date.now();
    try {
      stats = await api.sampleStats(server.id);
    } catch {
      stats = null;
    }
  }
  function refreshAll() {
    void refreshLive();
    void refreshStats();
  }

  /**
   * État d'exécution. `GET /server/{id}/live/light` renvoie `status: null` (vérifié le
   * 2026-08-19) : la seule source fiable est l'échantillon WebSocket (`stats.state`), d'où
   * l'ordre. `live.status` reste en second au cas où l'API se mette à le renseigner.
   */
  const runState = $derived(stats?.state ?? live?.status ?? null);
  const meta = $derived(runtimeMeta(runState, server.status));

  /**
   * Relevés : soutenus sur l'Aperçu (jauges), espacés ailleurs, où seuls la pastille et les
   * actions d'alimentation de l'en-tête en dépendent. Chaque échantillon ouvre une connexion
   * éphémère : inutile de les multiplier.
   */
  $effect(() => {
    const onOverview = current === "overview";
    if (Date.now() - lastStats > 3000) refreshAll();
    const a = setInterval(refreshLive, onOverview ? 8000 : 30000);
    const b = setInterval(refreshStats, onOverview ? 4000 : 15000);
    return () => {
      clearInterval(a);
      clearInterval(b);
    };
  });

  /** Après une action d'alimentation : l'état met un instant à changer côté hébergeur. */
  function afterPower() {
    setTimeout(refreshAll, 1500);
  }

  // --- Barre du haut : transparente sur la bannière de l'Aperçu, pleine ailleurs ---

  let heroH = $state(0);
  let topH = $state(0);
  let ovScroll = $state(0);
  const solid = $derived(current !== "overview" || (heroH > 0 && ovScroll > heroH - topH - 6));
  let ovPage: HTMLElement | null = null;

  /** Le titre de la barre pleine ramène en haut de l'Aperçu (bannière, alimentation). */
  function toOverviewTop() {
    goto("overview");
    ovPage?.scrollTo({ top: 0, behavior: "smooth" });
  }

  /** Repère la page de l'Aperçu (défilement de la bannière, retour en haut). */
  function overviewPage(node: HTMLElement, id: string) {
    if (id === "overview") ovPage = node;
    return {
      destroy() {
        if (ovPage === node) ovPage = null;
      },
    };
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

<div class="shell biome" style={biomeVars(biome)}>
  <!-- Barre du haut, posée sur les pages : transparente sur la bannière de l'Aperçu (dont elle
       laisse voir le paysage), pleine avec un fin paysage partout ailleurs. -->
  <header class="topbar" class:solid bind:clientHeight={topH}>
    <div class="tb-land" style="background: {biome.sky}" aria-hidden="true">
      {#if topH > 0}<BiomeLandscape {biome} seed={server.id} height={topH} pixel={4} />{/if}
    </div>
    <button class="tb-btn" onclick={onBack} aria-label={t("common.back")}>
      <Icon name="back" size={22} />
    </button>
    <!-- Nom sur une puce sombre : lisible sur tous les paysages (arbres, sapins enneigés). -->
    <span class="tb-slot">
      <button
        class="tb-title"
        onclick={toOverviewTop}
        tabindex={solid ? 0 : -1}
        aria-hidden={!solid}
        aria-label={`${server.name} · ${t(meta.key)} · ${t("nav.overview")}`}
      >
        <StatusDot color={meta.color} pulse={runState === "running"} size={8} />
        <span class="tb-name">{server.name}</span>
      </button>
    </span>
    <AnonToggle tone="banner" />
    {#if solid}
      <PowerControl
        variant="icon"
        serverId={server.id}
        {runState}
        restartWarning={server.restart_warning}
        stopWarning={server.stop_warning}
        onDone={afterPower}
      />
    {:else}
      <button class="tb-btn" onclick={() => (pickerOpen = true)} aria-label={t("biome.choose")}>
        <BiomeBlock {biome} size={20} />
      </button>
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
        <section
          class="page"
          class:under={tab.id === "overview"}
          use:overviewPage={tab.id}
          onscroll={tab.id === "overview" ? (e) => (ovScroll = e.currentTarget.scrollTop) : undefined}
        >
          {#if tab.id === "overview"}
            <ServerHero {server} {biome} {runState} onPowerDone={afterPower} bind:height={heroH} />
            <OverviewView
              {server}
              {mybox}
              {live}
              {stats}
              error={liveError}
              active={current === "overview"}
              onNavigate={goto}
            />
          {:else if tab.id === "console"}
            <ConsoleView
              serverId={server.id}
              active={current === "console"}
              onFocusChange={(f) => (navHidden = f)}
            />
          {:else if tab.id === "players"}
            <PlayersView
              serverId={server.id}
              active={current === "players"}
              playerData={caps?.player_data ?? false}
            />
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

  {#if moreOpen}
    <div
      class="scrim"
      use:portal
      role="button"
      tabindex="-1"
      aria-label={t("common.close")}
      onclick={() => (moreOpen = false)}
      onkeydown={(e) => e.key === "Escape" && (moreOpen = false)}
    ></div>
    <div class="sheet" use:portal role="menu" aria-label={t("nav.moreTitle")}>
      <div class="sheet-handle"></div>
      <div class="sheet-title">{t("nav.moreTitle")}</div>
      {#each overflow as tab (tab.id)}
        <button role="menuitem" class:on={tab.id === current} onclick={() => goto(tab.id)}>
          <span class="mg"><Icon name={tab.icon} size={20} /></span>
          <span>{tab.label}</span>
          {#if tab.id === current}<span class="here"><Icon name="check" size={18} /></span>{/if}
        </button>
      {/each}
    </div>
  {/if}

  {#if pickerOpen}
    <BiomePicker serverId={server.id} onClose={() => (pickerOpen = false)} />
  {/if}
</div>

<style>
  .shell {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100dvh;
    overflow: hidden;
  }

  /* --- Barre du haut ---------------------------------------------------------- */
  .topbar {
    position: absolute;
    z-index: 20;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: calc(var(--safe-top) + 6px) 10px 6px;
    min-height: calc(var(--safe-top) + 56px);
    /* Sur le paysage : couleurs du thème sombre dans les deux thèmes. */
    color: #f1f5f3;
    --state-running: #5fd68c;
    --state-pending: #f2b84b;
    --state-offline: #6b7a7f;
    --state-danger: #ff6b63;
    --state-hibernate: #c79bff;
    --text-dim: #8c9a94;
  }
  .tb-land {
    position: absolute;
    inset: 0;
    z-index: -1;
    opacity: 0;
    transition: opacity 0.18s ease;
  }
  .topbar.solid .tb-land {
    opacity: 1;
  }
  .tb-btn {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    padding: 0;
    border: none;
    border-radius: var(--radius);
    background: rgba(13, 17, 19, 0.72);
    color: #e4ebe8;
  }
  .tb-slot {
    flex: 1;
    min-width: 0;
    display: flex;
  }
  .tb-title {
    min-width: 0;
    max-width: 100%;
    display: inline-flex;
    align-items: center;
    gap: 9px;
    height: 44px;
    padding: 0 14px 0 13px;
    border: none;
    border-radius: var(--radius);
    background: rgba(13, 17, 19, 0.72);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.18s ease;
  }
  .topbar.solid .tb-title {
    opacity: 1;
    pointer-events: auto;
  }
  .tb-name {
    font-size: 17px;
    font-weight: 900;
    color: #ffffff;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* --- Pages --------------------------------------------------------------------- */
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
  @media (prefers-reduced-motion: reduce) {
    .track {
      transition: none;
    }
  }
  /* Les vues défilent sous la barre du haut (posée par-dessus) : leur zone de défilement commence
     juste en dessous, pour que leurs barres collantes (`sticky; top: 0`) s'y arrêtent. */
  .page {
    flex: 0 0 100%;
    width: 100%;
    margin-top: calc(var(--safe-top) + 56px);
    height: calc(100% - var(--safe-top) - 56px);
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    padding-bottom: calc(var(--nav-height) + var(--safe-bottom));
  }
  /* L'Aperçu commence par la bannière, qui passe sous la barre du haut transparente. */
  .page.under {
    margin-top: 0;
    height: 100%;
  }
  .pages.nav-hidden .page {
    padding-bottom: 0;
  }

  /* --- Feuille « Plus » ------------------------------------------------------------ */
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
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
    padding: 8px 10px calc(var(--safe-bottom) + 14px);
    background: var(--elevated);
    border-top-left-radius: var(--radius-lg);
    border-top-right-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    color: var(--text);
  }
  .sheet-title {
    padding: 4px 12px 8px;
    color: var(--text-dim);
    font-size: 13px;
    font-weight: 800;
  }
  .sheet button {
    display: flex;
    align-items: center;
    gap: 14px;
    min-height: 54px;
    padding: 0 12px;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text);
    font-size: 16px;
    font-weight: 800;
    text-align: left;
  }
  .mg {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: var(--radius);
    background: var(--inset);
    color: var(--text-muted);
  }
  /* Vue ouverte : fond teinté du biome. */
  .sheet button.on {
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .sheet button.on .mg {
    background: var(--accent);
    color: var(--on-accent);
  }
  .here {
    display: inline-flex;
    margin-left: auto;
  }
</style>
