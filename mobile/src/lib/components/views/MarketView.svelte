<script lang="ts">
  /**
   * Marketplace Minecraft (mods & plugins) — pendant tactile de la vue desktop.
   *
   * Catalogue et installation passent par l'API MineStrator (Modrinth, CurseForge, SpigotMC) : ni
   * SFTP ni mot de passe SFTP. L'installation directe n'existe que pour Modrinth et SpigotMC ; un
   * projet CurseForge, payant ou à téléchargement externe renvoie vers sa page. Toute
   * installation ne prend effet qu'au prochain démarrage du serveur.
   *
   * Monté en permanence dans le pager : rien n'est chargé avant le premier affichage de l'onglet.
   */
  import { api, humanizeError, openExternal } from "../../ipc";
  import { t } from "../../i18n";
  import { portal } from "../../portal";
  import Icon from "../Icon.svelte";
  import type {
    InstalledItem,
    MarketItem,
    MarketKind,
    MarketSource,
    MarketVersion,
  } from "../../types";

  let { serverId, active }: { serverId: number; active: boolean } = $props();

  // Sources par type de contenu et loaders proposés (mêmes listes que le desktop).
  const SOURCES: Record<MarketKind, MarketSource[]> = {
    mods: ["modrinth", "curseforge"],
    plugins: ["modrinth", "spigot"],
  };
  const LOADERS: Record<MarketKind, string[]> = {
    mods: ["fabric", "neoforge", "forge", "quilt"],
    plugins: ["paper", "spigot", "purpur", "folia", "bukkit"],
  };
  const SOURCE_LABEL: Record<string, string> = {
    modrinth: "Modrinth",
    curseforge: "CurseForge",
    spigot: "SpigotMC",
  };
  /** Taille de page des catalogues (fixée par l'API). */
  const PAGE_SIZE = 18;

  let kind = $state<MarketKind>("mods");
  let source = $state<MarketSource>("modrinth");
  let loader = $state("fabric");
  let gameVersion = $state("");
  let query = $state("");
  let page = $state(1);

  let items = $state<MarketItem[]>([]);
  let totalHits = $state<number | null>(null);
  let lastPageSize = $state(0);
  let loading = $state(false);
  let loadingMore = $state(false);
  let error = $state<string | null>(null);

  let started = $state(false);
  let mcVersions = $state<string[]>([]);
  let installed = $state<InstalledItem[]>([]);
  let showInstalled = $state(false);
  /** Installés depuis l'ouverture de la vue (le catalogue ne le sait pas). */
  let installedIds = $state<Set<string>>(new Set());
  let notice = $state<string | null>(null);

  /** Feuille d'installation d'un projet. */
  let picking = $state<{
    item: MarketItem;
    versions: MarketVersion[];
    selected: string;
    loading: boolean;
    error: string | null;
  } | null>(null);
  let installing = $state(false);

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  /** Numéro de requête : une réponse plus ancienne que la dernière demande est ignorée. */
  let seq = 0;

  $effect(() => () => {
    clearTimeout(searchTimer);
    clearTimeout(noticeTimer);
  });

  // Premier affichage : détection du type de contenu et du loader depuis ce qui est installé.
  $effect(() => {
    if (active && !started) void start();
  });

  async function start() {
    started = true;
    const [mods, plugins, versions] = await Promise.all([
      api.installedMods(serverId).catch(() => [] as InstalledItem[]),
      api.installedPlugins(serverId).catch(() => [] as InstalledItem[]),
      api.marketMinecraftVersions().catch(() => [] as string[]),
    ]);
    mcVersions = versions;
    if (mods.length > 0) {
      kind = "mods";
      loader = normalizeLoader(mods[0].loader) ?? "fabric";
    } else if (plugins.length > 0) {
      kind = "plugins";
      loader = "paper";
    }
    installed = kind === "mods" ? mods : plugins;
    await load();
  }

  function normalizeLoader(l: string): string | null {
    const x = l.toLowerCase();
    return LOADERS.mods.includes(x) ? x : null;
  }

  async function loadInstalled() {
    try {
      installed =
        kind === "mods" ? await api.installedMods(serverId) : await api.installedPlugins(serverId);
    } catch {
      installed = [];
    }
  }

  /** Première page des filtres courants (remplace la liste). */
  async function load() {
    const my = ++seq;
    loading = true;
    error = null;
    page = 1;
    try {
      const p = await api.marketList(kind, source, 1, query.trim(), loader, gameVersion);
      if (my !== seq) return;
      items = p.items;
      totalHits = p.total_hits;
      lastPageSize = p.items.length;
    } catch (e) {
      if (my !== seq) return;
      error = humanizeError(e);
      items = [];
      totalHits = null;
    } finally {
      if (my === seq) loading = false;
    }
  }

  /** Page suivante, ajoutée à la liste (défilement continu plutôt que pagination). */
  async function loadMore() {
    if (loadingMore || loading) return;
    const my = seq;
    loadingMore = true;
    try {
      const p = await api.marketList(kind, source, page + 1, query.trim(), loader, gameVersion);
      if (my !== seq) return;
      const known = new Set(items.map((i) => i.id));
      items = [...items, ...p.items.filter((i) => !known.has(i.id))];
      lastPageSize = p.items.length;
      page += 1;
    } catch (e) {
      if (my === seq) error = humanizeError(e);
    } finally {
      loadingMore = false;
    }
  }

  const hasMore = $derived(
    totalHits != null ? page * PAGE_SIZE < totalHits : lastPageSize >= PAGE_SIZE,
  );

  function setKind(k: MarketKind) {
    if (k === kind) return;
    kind = k;
    source = "modrinth";
    loader = LOADERS[k][0];
    void loadInstalled();
    void load();
  }

  function onSearchInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void load(), 400);
  }

  function flash(msg: string) {
    notice = msg;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 4000);
  }

  /** Modrinth et SpigotMC s'installent directement ; le reste renvoie vers la page du projet. */
  function installable(item: MarketItem): boolean {
    return (source === "modrinth" || source === "spigot") && !item.premium && !item.external;
  }

  async function openInstall(item: MarketItem) {
    picking = { item, versions: [], selected: "", loading: true, error: null };
    try {
      const key = source === "modrinth" ? item.slug || item.id : item.id;
      const versions = await api.marketVersions(source, key, loader, gameVersion);
      if (picking?.item.id !== item.id) return;
      picking.versions = versions;
      picking.selected = versions[0]?.id ?? "";
      if (versions.length === 0) picking.error = t("market.noVersions");
    } catch (e) {
      if (picking?.item.id === item.id) picking.error = humanizeError(e);
    } finally {
      if (picking?.item.id === item.id) picking.loading = false;
    }
  }

  async function confirmInstall() {
    if (!picking || !picking.selected || installing) return;
    const { item, selected } = picking;
    installing = true;
    picking.error = null;
    try {
      await api.installMod(
        serverId,
        source,
        kind === "plugins" ? "plugin" : "mod",
        item.slug || item.id,
        selected,
        loader,
      );
      installedIds = new Set(installedIds).add(item.id);
      picking = null;
      flash(`${item.name} — ${t("market.installOk")}`);
      void loadInstalled();
    } catch (e) {
      if (picking) picking.error = humanizeError(e);
    } finally {
      installing = false;
    }
  }

  function fmtCount(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)} M`;
    if (n >= 1000) return `${Math.round(n / 1000)} k`;
    return `${n}`;
  }
  function fmtSize(n: number | null): string {
    if (!n || n <= 0) return "";
    return n >= 1024 ** 2 ? `${(n / 1024 ** 2).toFixed(1)} Mo` : `${Math.max(1, Math.round(n / 1024))} Ko`;
  }
</script>

<div class="view">
  <header class="head">
    <h2>{t("market.title")}</h2>
    <div class="seg" role="group" aria-label={t("market.title")}>
      <button class:on={kind === "mods"} onclick={() => setKind("mods")}>{t("market.mods")}</button>
      <button class:on={kind === "plugins"} onclick={() => setKind("plugins")}>
        {t("market.plugins")}
      </button>
    </div>
  </header>

  <div class="top">
    <div class="search">
      <Icon name="search" size={16} />
      <input
        type="search"
        bind:value={query}
        oninput={onSearchInput}
        placeholder={t("market.search")}
        aria-label={t("market.search")}
        autocapitalize="off"
        spellcheck="false"
      />
    </div>
    <!-- Sélecteurs natifs : sur Android, ils ouvrent la liste de choix du système. La rangée
         défile à l'horizontale : `data-noswipe` l'exclut du changement d'onglet par balayage. -->
    <div class="filters" role="group" aria-label={t("market.filters")} data-noswipe>
      <label class="pick">
        <span class="sr">{t("market.source")}</span>
        <select bind:value={source} onchange={load}>
          {#each SOURCES[kind] as s (s)}<option value={s}>{SOURCE_LABEL[s]}</option>{/each}
        </select>
        <Icon name="chevronDown" size={14} />
      </label>
      <label class="pick">
        <span class="sr">{t("market.loader")}</span>
        <select bind:value={loader} onchange={load}>
          {#each LOADERS[kind] as l (l)}<option value={l}>{l}</option>{/each}
        </select>
        <Icon name="chevronDown" size={14} />
      </label>
      <label class="pick">
        <span class="sr">{t("market.gameVersion")}</span>
        <select bind:value={gameVersion} onchange={load}>
          <option value="">{t("market.allVersions")}</option>
          {#each mcVersions as v (v)}<option value={v}>{v}</option>{/each}
        </select>
        <Icon name="chevronDown" size={14} />
      </label>
    </div>
  </div>

  {#if notice}<p class="ok"><Icon name="check" size={15} /> {notice}</p>{/if}

  <section class="installed">
    <button class="sec" onclick={() => (showInstalled = !showInstalled)} aria-expanded={showInstalled}>
      <span class="caret" class:open={showInstalled}><Icon name="chevronRight" size={16} /></span>
      <span>{t("market.installed")}</span>
      <span class="badge">{installed.length}</span>
    </button>
    {#if showInstalled}
      {#if installed.length === 0}
        <p class="dim small">{t("market.installedEmpty")}</p>
      {:else}
        <ul class="inst">
          {#each installed as m (m.filename || m.name)}
            <li class:off={!m.enabled}>
              <span class="i-name selectable">{m.name}</span>
              <span class="i-meta dim">
                {[m.version, m.loader].filter(Boolean).join(" · ")}
                {#if !m.enabled}· {t("market.disabled")}{/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </section>

  <section class="catalog">
    <div class="sec static">
      <span>{t("market.catalog")}</span>
      {#if totalHits != null}<span class="dim small">{fmtCount(totalHits)} {t("market.results")}</span>{/if}
    </div>

    {#if error && items.length === 0}
      <div class="state">
        <p class="err selectable">{error}</p>
        <button class="ghost" onclick={load}>{t("props.retry")}</button>
      </div>
    {:else if loading || !started}
      <p class="dim">…</p>
    {:else if items.length === 0}
      <p class="dim">{t("market.noResults")}</p>
    {:else}
      <ul class="cards">
        {#each items as item (item.source + item.id)}
          <li class="card">
            <div class="c-top">
              {#if item.icon_url}
                <img class="c-ic" src={item.icon_url} alt="" loading="lazy" />
              {:else}
                <span class="c-ic ph"><Icon name="package" size={20} /></span>
              {/if}
              <div class="c-meta">
                <span class="c-name">
                  {item.name}
                  {#if item.premium}<span class="prem">{t("market.premium")}</span>{/if}
                </span>
                <span class="c-sub dim">
                  <Icon name="download" size={12} />
                  {fmtCount(item.downloads)}
                  {#if item.loaders.length > 0}· {item.loaders.slice(0, 3).join(", ")}{/if}
                  {#if item.game_versions.length > 0}· {item.game_versions[0]}{/if}
                </span>
              </div>
            </div>
            {#if item.tag}<p class="c-tag">{item.tag}</p>{/if}
            <div class="c-act">
              {#if installable(item)}
                {#if installedIds.has(item.id)}
                  <span class="done"><Icon name="check" size={15} /> {t("market.installedOne")}</span>
                {:else}
                  <button class="primary" onclick={() => openInstall(item)}>
                    <Icon name="download" size={16} />
                    {t("market.install")}
                  </button>
                {/if}
              {:else if item.external_url}
                <button class="ghost" onclick={() => openExternal(item.external_url!)}>
                  <Icon name="external" size={15} />
                  {t("market.viewOn")} {SOURCE_LABEL[item.source] ?? item.source}
                </button>
              {:else}
                <span class="dim small">{t("market.installUnsupported")}</span>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
      {#if error}<p class="err selectable">{error}</p>{/if}
      {#if hasMore}
        <button class="ghost more" onclick={loadMore} disabled={loadingMore}>
          {loadingMore ? "…" : t("market.loadMore")}
        </button>
      {/if}
    {/if}
  </section>
</div>

{#if picking}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => !installing && (picking = null)}
    onkeydown={(e) => e.key === "Escape" && !installing && (picking = null)}
  ></div>
  <div class="sheet" use:portal>
    <div class="s-head">
      <span class="s-title">{picking.item.name}</span>
      <span class="dim small">{t("market.chooseVersion")}</span>
    </div>
    {#if picking.loading}
      <p class="dim">{t("market.loadingVersions")}</p>
    {:else if picking.versions.length > 0}
      <div class="versions" role="radiogroup" aria-label={t("market.chooseVersion")}>
        {#each picking.versions as v (v.id)}
          <button
            class="ver"
            role="radio"
            aria-checked={picking.selected === v.id}
            class:on={picking.selected === v.id}
            onclick={() => picking && (picking.selected = v.id)}
          >
            <span class="radio"></span>
            <span class="v-body">
              <span class="v-name">{v.version_number || v.name || v.id}</span>
              <span class="v-sub dim">
                {[v.release_type, v.game_versions.slice(0, 3).join(", "), fmtSize(v.file_size)]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </span>
          </button>
        {/each}
      </div>
    {/if}
    {#if picking.error}<p class="err selectable">{picking.error}</p>{/if}
    <div class="s-acts">
      <button class="ghost" onclick={() => (picking = null)} disabled={installing}>
        {t("market.cancel")}
      </button>
      <button
        class="primary"
        onclick={confirmInstall}
        disabled={installing || picking.loading || !picking.selected}
      >
        {installing ? t("market.installing") : t("market.confirm")}
      </button>
    </div>
  </div>
{/if}

<style>
  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 700;
  }
  .seg {
    display: inline-flex;
    padding: 2px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
  }
  .seg button {
    padding: 0 14px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: var(--text-dim);
    font-size: 14px;
  }
  .seg button.on {
    background: color-mix(in srgb, var(--brand-primary) 18%, transparent);
    color: var(--text);
    font-weight: 600;
  }

  /* Recherche et filtres collés en haut pendant le défilement du catalogue. */
  .top {
    position: sticky;
    top: 0;
    z-index: 3;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0 -16px;
    padding: 8px 16px;
    background: color-mix(in srgb, var(--bg) 94%, transparent);
    backdrop-filter: blur(8px);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-dim);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 44px;
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 16px; /* 16 px : évite le zoom automatique d'Android au focus */
    outline: none;
  }
  .filters {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .pick {
    position: relative;
    display: inline-flex;
    align-items: center;
    flex: none;
    color: var(--text-dim);
  }
  .pick select {
    appearance: none;
    height: 44px;
    max-width: 46vw;
    padding: 0 28px 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 14px;
    text-overflow: ellipsis;
  }
  .pick :global(svg) {
    position: absolute;
    right: 10px;
    pointer-events: none;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .sec {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 0 2px;
    border: 0;
    background: none;
    color: var(--text-muted);
    font-size: 14px;
    font-weight: 600;
    text-align: left;
  }
  .sec.static {
    min-height: 32px;
    justify-content: space-between;
  }
  .caret {
    display: inline-flex;
    transition: transform 0.15s;
  }
  .caret.open {
    transform: rotate(90deg);
  }
  .badge {
    padding: 1px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 8%, transparent);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .inst {
    list-style: none;
    margin: 4px 0 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
  }
  .inst li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 12px;
  }
  .inst li + li {
    border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .inst li.off {
    opacity: 0.6;
  }
  .i-name {
    font-size: 14px;
    overflow-wrap: anywhere;
  }
  .i-meta {
    font-size: 12px;
  }

  .cards {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .c-top {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .c-ic {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 8px;
    object-fit: cover;
    background: var(--elevated);
  }
  .c-ic.ph {
    display: grid;
    place-items: center;
    color: var(--text-dim);
  }
  .c-meta {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .c-name {
    font-size: 15px;
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .prem {
    margin-left: 6px;
    padding: 1px 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--brand-tertiary) 22%, transparent);
    color: var(--text);
    font-size: 11px;
    font-weight: 600;
    vertical-align: middle;
  }
  .c-sub {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px;
    font-size: 12px;
  }
  .c-tag {
    margin: 0;
    font-size: 13px;
    line-height: 1.4;
    color: var(--text-muted);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .c-act {
    display: flex;
    justify-content: flex-end;
  }
  .done {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 44px;
    color: var(--brand-primary);
    font-weight: 600;
    font-size: 14px;
  }
  .more {
    width: 100%;
  }

  .primary,
  .ghost {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 0 16px;
    border-radius: var(--radius-sm);
    font-weight: 600;
    font-size: 14px;
  }
  .primary {
    border: none;
    background: var(--brand-primary);
    color: #fff;
  }
  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
  }
  .primary:disabled,
  .ghost:disabled {
    opacity: 0.5;
  }

  .state {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 12.5px;
  }
  .ok {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    color: var(--brand-primary);
    font-size: 14px;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
    font-size: 13px;
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
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px calc(var(--safe-bottom) + 16px);
    background: var(--elevated);
    border-top-left-radius: 16px;
    border-top-right-radius: 16px;
  }
  .s-head {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .s-title {
    font-size: 16px;
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .versions {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .ver {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 52px;
    padding: 8px 12px;
    border: 0;
    background: none;
    text-align: left;
  }
  .ver + .ver {
    border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .ver.on {
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
  }
  .radio {
    width: 18px;
    height: 18px;
    flex: none;
    border-radius: 50%;
    border: 2px solid var(--text-dim);
  }
  .ver.on .radio {
    border-color: var(--brand-primary);
    box-shadow: inset 0 0 0 3px var(--elevated);
    background: var(--brand-primary);
  }
  .v-body {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .v-name {
    font-size: 14px;
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .v-sub {
    font-size: 12px;
  }
  .s-acts {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
