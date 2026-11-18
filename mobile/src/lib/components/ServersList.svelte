<script lang="ts">
  import { api, humanizeError } from "../ipc";
  import { auth } from "../stores/auth.svelte";
  import { alerts } from "../stores/alerts.svelte";
  import { applyAccountSwitch } from "../stores/accounts.svelte";
  import { t } from "../i18n";
  import { biomeVars, serverBiome } from "../servers/colors.svelte";
  import { statusMeta } from "../status";
  import Icon from "./Icon.svelte";
  import GameIcon from "./GameIcon.svelte";
  import Logo from "./Logo.svelte";
  import AnonToggle from "./AnonToggle.svelte";
  import BiomeLandscape from "./BiomeLandscape.svelte";
  import Masked from "./Masked.svelte";
  import StatusDot from "./StatusDot.svelte";
  import MyBoxResources from "./MyBoxResources.svelte";
  import type { MyBoxCapacity, MyBoxSummary, ServerListItem } from "../types";

  let {
    onOpen,
    onSettings,
    onAlerts,
  }: {
    onOpen: (server: ServerListItem, box: MyBoxSummary | null) => void;
    onSettings: () => void;
    /** Centre d'alertes (qui mène aussi aux notifications de l'hébergeur), comme la cloche desktop. */
    onAlerts: () => void;
  } = $props();

  let myboxes = $state<MyBoxSummary[]>([]);
  let servers = $state<ServerListItem[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** Joueurs en ligne par serveur — un seul appel groupé pour toute la liste. */
  let players = $state<Record<number, { current: number; limit: number }>>({});

  /**
   * Ressources des MyBox : un seul appel pour toutes, fait à la première ouverture d'une carte
   * « Ressources » (plusieurs requêtes par MyBox : inutile tant que personne ne les consulte).
   */
  let caps = $state<MyBoxCapacity[] | null>(null);
  let capsLoading = $state(false);
  let capsError = $state<string | null>(null);
  /** Cartes « Ressources » dépliées, par identifiant de MyBox. */
  let openRes = $state<Record<number, boolean>>({});

  /** `silent` : rafraîchit sans repasser par l'état de chargement (après un renommage). */
  async function load(silent = false) {
    if (!silent) {
      loading = true;
      error = null;
    }
    try {
      const overview = await api.listServers();
      myboxes = overview.myboxes;
      servers = overview.servers;
      void loadPlayers();
    } catch (err) {
      // En silencieux, la liste déjà affichée reste valable : on ne la remplace pas par l'erreur.
      if (!silent) error = humanizeError(err);
    } finally {
      loading = false;
    }
  }

  async function loadCaps() {
    capsLoading = true;
    capsError = null;
    try {
      caps = await api.myboxCapacities();
    } catch (err) {
      capsError = humanizeError(err);
    } finally {
      capsLoading = false;
    }
  }

  function toggleRes(boxId: number) {
    openRes[boxId] = !openRes[boxId];
    if (openRes[boxId] && caps === null && !capsLoading) void loadCaps();
  }

  /** Après un renommage : capacités et liste relues (noms des groupes et des serveurs). */
  async function afterRename() {
    await Promise.all([loadCaps(), load(true)]);
  }

  /** Best-effort : un compte de joueurs manquant ne doit pas dégrader la liste. */
  async function loadPlayers() {
    const ids = servers.filter((s) => s.status === "active").map((s) => s.id);
    if (ids.length === 0) return;
    try {
      const rows = await api.livePlayersBulk(ids);
      players = Object.fromEntries(
        rows.map((r) => [r.server_id, { current: r.current, limit: r.limit }]),
      );
    } catch {
      players = {};
    }
  }

  /** Retire le compte actif : bascule sur un autre compte s'il en reste, sinon écran de connexion. */
  async function doLogout() {
    try {
      await applyAccountSwitch(await api.logout());
    } catch {
      auth.setUser(null);
    }
  }

  // Groupes MyBox (ordre des myboxes ; serveurs orphelins en fin).
  const groups = $derived.by(() => {
    const byBox = new Map<number, ServerListItem[]>();
    for (const s of servers) {
      if (!byBox.has(s.mybox_id)) byBox.set(s.mybox_id, []);
      byBox.get(s.mybox_id)!.push(s);
    }
    const out: { box: MyBoxSummary | null; servers: ServerListItem[] }[] = [];
    for (const b of myboxes) {
      const list = byBox.get(b.id) ?? [];
      if (list.length) out.push({ box: b, servers: list });
      byBox.delete(b.id);
    }
    const leftover: ServerListItem[] = [];
    for (const list of byBox.values()) leftover.push(...list);
    if (leftover.length) out.push({ box: null, servers: leftover });
    return out;
  });

  function boxBadge(b: MyBoxSummary): { text: string; cls: string } {
    if (b.expired) return { text: t("mybox.state.expired"), cls: "danger" };
    if (b.suspended) return { text: t("mybox.state.suspended"), cls: "warn" };
    if (b.days_left <= 7) return { text: `${b.days_left} j`, cls: "warn" };
    return { text: `${b.days_left} j`, cls: "dim" };
  }

  $effect(() => {
    load();
  });
</script>

<header class="top">
  <span class="mark" aria-hidden="true"><Logo size={24} /></span>
  <div class="titles">
    <h1>{t("servers.title")}</h1>
    {#if auth.user}<span class="who selectable">{auth.user.pseudo}</span>{/if}
  </div>
  <div class="actions">
    <AnonToggle />
    <button class="ic bell" onclick={onAlerts} aria-label={t("alerts.title")}>
      <Icon name="bell" size={20} />
      {#if alerts.unread > 0}
        <span class="badge">{alerts.unread > 9 ? "9+" : alerts.unread}</span>
      {/if}
    </button>
    <button class="ic" onclick={onSettings} aria-label={t("settings.title")}>
      <Icon name="settings" size={20} />
    </button>
    <button class="ic" onclick={doLogout} aria-label={t("logout")}>
      <Icon name="logout" size={20} />
    </button>
  </div>
</header>

<div class="body">
  {#if loading}
    <div class="state"><span class="spinner"></span></div>
  {:else if error}
    <p class="alert selectable">{error}</p>
    <button class="btn btn--ghost" onclick={() => load()}>
      <Icon name="refresh" size={16} /> {t("servers.refresh")}
    </button>
  {:else if servers.length === 0}
    <p class="dim">{t("servers.empty")}</p>
  {:else}
    {#each groups as g (g.box?.id ?? "other")}
      <section class="group">
        {#if g.box}
          {@const badge = boxBadge(g.box)}
          {@const boxId = g.box.id}
          <div class="ghead">
            <span class="gname">{g.box.name}</span>
            <span class="goffer">{g.box.offer}</span>
            <span class="gbadge {badge.cls}">{badge.text}</span>
          </div>
          <!-- Ressources de la MyBox : carte repliable, chargée à la première ouverture. -->
          <div class="res" class:open={openRes[boxId]}>
            <button
              class="restoggle"
              aria-expanded={!!openRes[boxId]}
              onclick={() => toggleRes(boxId)}
            >
              <span class="rg"><Icon name="sliders" size={16} /></span>
              <span class="rlabel">{t("mybox.resources")}</span>
              <span class="chev"><Icon name="chevronDown" size={18} /></span>
            </button>
            {#if openRes[boxId]}
              {@const cap = caps?.find((c) => c.id === boxId)}
              <div class="resbody">
                {#if cap}
                  <MyBoxResources {cap} onChanged={afterRename} />
                {:else if capsLoading}
                  <p class="dim small">{t("mybox.loading")}</p>
                {:else if capsError}
                  <p class="alert small selectable">{capsError}</p>
                  <button class="btn btn--ghost" onclick={() => loadCaps()}>
                    <Icon name="refresh" size={16} /> {t("mybox.retry")}
                  </button>
                {:else}
                  <p class="dim small">{t("mybox.unavailable")}</p>
                {/if}
              </div>
            {/if}
          </div>
        {:else}
          <div class="ghead"><span class="gname">{t("servers.other")}</span></div>
        {/if}
        <ul>
          {#each g.servers as s (s.id)}
            {@const biome = serverBiome(s.id)}
            {@const meta = statusMeta(s.status)}
            <li class="card srv biome" style={biomeVars(biome)}>
              <button class="cardbtn" onclick={() => onOpen(s, g.box)} aria-label={t("servers.open").replace("{name}", s.name)}>
                <!-- Fine bannière : le paysage du biome du serveur (même relief que son en-tête). -->
                <span class="cover">
                  <BiomeLandscape {biome} seed={s.id} height={58} pixel={4} />
                  <span class="state" style="color: {meta.ink}">
                    <StatusDot color={meta.color} size={7} />{t(meta.key)}
                  </span>
                </span>
                <span class="head">
                  <!-- L'icône du jeu chevauche le bas de la bannière. -->
                  <span class="gicon"><GameIcon src={s.egg_icon_url} name={s.egg_name} size={38} radius={7} /></span>
                  <span class="titles">
                    <span class="name">{s.name}</span>
                    <span class="addr"><Masked value={s.address} kind="address" /></span>
                  </span>
                  <span class="go"><Icon name="chevronRight" size={20} /></span>
                </span>
                <span class="meta">
                  <span class="chip">{s.egg_name}{s.bedrock ? " · Bedrock" : ""}</span>
                  {#if players[s.id] && s.capabilities?.players}
                    <span class="chip pl">
                      <Icon name="players" size={13} /><span class="mono">{players[s.id].current}/{players[s.id].limit}</span>
                    </span>
                  {/if}
                  {#if !s.owner}<span class="chip sub">{t("servers.subUser")}</span>{/if}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</div>

<style>
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: calc(var(--safe-top) + 14px) 16px 12px;
  }
  /* Tuile du logo : toujours sombre (DESIGN.md). */
  .mark {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    flex: none;
    border-radius: 10px;
    background: #0d1113;
  }
  .titles {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  h1 {
    margin: 0;
    font-size: 23px;
    line-height: 1.15;
  }
  .who {
    color: var(--text-dim);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 4px;
    flex: none;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    padding: 0;
    background: var(--surface);
    border: 1px solid transparent;
    border-radius: var(--radius);
    color: var(--text-muted);
  }
  .bell {
    position: relative;
  }
  /* Alertes non lues : même pastille que la cloche desktop. */
  .badge {
    position: absolute;
    top: -4px;
    right: -4px;
    min-width: 18px;
    height: 18px;
    padding: 0 4px;
    border-radius: 999px;
    background: var(--state-danger);
    color: var(--bg);
    font-size: 11px;
    font-weight: 900;
    line-height: 18px;
    text-align: center;
    box-shadow: 0 0 0 2px var(--bg);
  }
  .body {
    padding: 4px 16px calc(var(--safe-bottom) + 24px);
  }
  .state {
    display: grid;
    place-items: center;
    padding: 40px 0;
  }
  .alert {
    margin: 0 0 12px;
    padding: 10px 13px;
    border-radius: var(--radius);
    background: var(--danger-bg);
    color: var(--danger-ink);
    font-size: 14px;
  }
  .alert.small {
    margin: 0;
    font-size: 13px;
  }
  .group {
    margin-bottom: 22px;
  }
  .ghead {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 6px 2px 10px;
    min-width: 0;
  }
  .gname {
    font-size: 15px;
    font-weight: 900;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .goffer {
    font-size: 12.5px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .gbadge {
    margin-left: auto;
    flex: none;
    font-size: 12px;
    font-weight: 800;
    padding: 2px 9px;
    border-radius: 999px;
    background: var(--inset);
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .gbadge.warn {
    color: var(--state-pending);
    background: color-mix(in srgb, var(--state-pending) 14%, var(--surface));
  }
  .gbadge.danger {
    color: var(--danger-ink);
    background: var(--danger-bg);
  }
  .res {
    margin-bottom: 10px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    overflow: hidden;
  }
  .restoggle {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 48px;
    padding: 0 12px;
    background: transparent;
    border: none;
    color: var(--text);
    font-size: 14.5px;
    font-weight: 800;
    text-align: left;
  }
  .rg {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    background: var(--inset);
    color: var(--text-muted);
  }
  .rlabel {
    flex: 1;
  }
  .chev {
    display: inline-flex;
    color: var(--text-muted);
    transition: transform 0.18s;
  }
  .res.open .chev {
    transform: rotate(180deg);
  }
  .resbody {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
    padding: 12px 14px 14px;
    border-top: 1px solid var(--border);
  }
  .resbody .btn {
    align-self: flex-start;
  }
  .small {
    margin: 0;
    font-size: 13px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* --- Carte serveur ---------------------------------------------------------------- */
  .srv {
    overflow: hidden;
  }
  .cardbtn {
    display: flex;
    flex-direction: column;
    width: 100%;
    padding: 0;
    border: none;
    background: none;
    color: var(--text);
    text-align: left;
  }
  .cardbtn:active {
    background: var(--hover);
  }
  .cover {
    position: relative;
    display: block;
    width: 100%;
  }
  /* Pastille d'état posée sur le paysage : couleurs du thème sombre dans les deux thèmes. */
  .state {
    --state-running: #5fd68c;
    --state-running-text: #8fe8b0;
    --state-pending: #f2b84b;
    --state-offline: #6b7a7f;
    --state-danger: #ff6b63;
    --state-hibernate: #c79bff;
    --text-dim: #8c9a94;
    position: absolute;
    top: 9px;
    right: 10px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border-radius: 999px;
    background: #0d1113;
    font-size: 12px;
    font-weight: 800;
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 0 12px 0 14px;
  }
  .gicon {
    position: relative; /* au-dessus du paysage (positionné) qu'elle chevauche */
    margin-top: -20px;
    padding: 3px;
    border-radius: 10px;
    background: var(--surface);
    flex: none;
  }
  .titles {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .head .titles {
    padding-top: 6px;
  }
  .name {
    font-weight: 900;
    font-size: 17px;
    line-height: 1.25;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .addr {
    font-family: var(--font-mono);
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .go {
    display: inline-flex;
    align-self: center;
    margin-top: 6px;
    color: var(--text-dim);
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 10px 14px 14px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12.5px;
    font-weight: 700;
    color: var(--text-muted);
    background: var(--inset);
    border-radius: 999px;
    padding: 3px 10px;
  }
  .chip.pl {
    color: var(--accent-text);
    background: var(--accent-soft);
  }
  .chip.pl .mono {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .chip.sub {
    color: var(--state-hibernate);
  }
  .dim {
    color: var(--text-dim);
  }
</style>
