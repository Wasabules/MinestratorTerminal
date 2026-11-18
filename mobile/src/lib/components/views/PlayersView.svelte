<script lang="ts">
  import { untrack } from "svelte";
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import type { KnownPlayer, LiveLight, PlayerAction, PlayerSort, PlayersPage } from "../../types";
  import PlayerSheet from "../PlayerSheet.svelte";

  /** `playerData` : joueurs déjà venus, fiche et inventaire (Minecraft Java, `GameCapabilities`). */
  let { serverId, active, playerData = false }: { serverId: number; active: boolean; playerData?: boolean } =
    $props();

  let live = $state<LiveLight | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state<string | null>(null); // clé "player:action" en cours
  let pseudo = $state("");

  // --- Joueurs déjà venus ---
  const PAGE = 30;
  const SORTS: PlayerSort[] = ["last_seen", "play_time", "name"];
  let known = $state<PlayersPage | null>(null);
  let knownList = $state<KnownPlayer[]>([]);
  let knownError = $state<string | null>(null);
  let knownLoading = $state(false);
  let search = $state("");
  let sort = $state<PlayerSort>("last_seen");
  let sheet = $state<KnownPlayer | null>(null);
  let seq = 0;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let loadedOnce = false;

  async function refresh() {
    try {
      live = await api.liveLight(serverId);
      error = null;
    } catch (err) {
      error = humanizeError(err);
    }
  }

  /** Charge une page de joueurs déjà venus ; `append` : « Afficher plus ». */
  async function loadKnown(append: boolean, refreshRoster = false) {
    const mine = ++seq;
    knownLoading = true;
    try {
      const offset = append ? knownList.length : 0;
      const page = await api.knownPlayers(
        serverId,
        { search, sort, filter: "all", offset, limit: PAGE },
        refreshRoster,
      );
      if (mine !== seq) return;
      known = page;
      if (append) {
        // La liste a pu changer entre deux pages (cache relu) : pas de doublon.
        const seen = new Set(knownList.map((p) => p.uuid));
        knownList = [...knownList, ...page.players.filter((p) => !seen.has(p.uuid))];
      } else {
        knownList = page.players;
      }
      knownError = null;
    } catch (err) {
      if (mine === seq) knownError = humanizeError(err);
    } finally {
      if (mine === seq) knownLoading = false;
    }
  }

  function onSearch(value: string) {
    search = value;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void loadKnown(false), 300);
  }

  function onSort(value: PlayerSort) {
    sort = value;
    void loadKnown(false);
  }

  async function act(player: string, action: PlayerAction) {
    const name = player.trim();
    if (name === "" || busy) return;
    busy = `${name}:${action}`;
    notice = null;
    try {
      await api.playerAction(serverId, action, name);
      notice = t("players.done");
      setTimeout(refresh, 800);
      // Drapeaux op/whitelist/ban relus par l'hébergeur : on lui laisse le temps de les écrire.
      if (playerData) setTimeout(() => void loadKnown(false, true), 1500);
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = null;
    }
  }

  // Actions disponibles pour un pseudo saisi (marche même hors-ligne).
  const byNameActions: { action: PlayerAction; key: string }[] = [
    { action: "ban", key: "action.ban" },
    { action: "unban", key: "action.unban" },
    { action: "op_add", key: "action.op" },
    { action: "op_remove", key: "action.deop" },
    { action: "whitelist_add", key: "action.wl_add" },
    { action: "whitelist_remove", key: "action.wl_remove" },
  ];

  function playTime(secs: number | null): string {
    if (secs === null) return "—";
    const m = Math.floor(secs / 60);
    if (m < 60) return `${m} min`;
    const h = Math.floor(m / 60);
    return h >= 100 ? `${h} h` : `${h} h ${String(m % 60).padStart(2, "0")}`;
  }

  function lastSeen(p: KnownPlayer): string {
    if (p.online) return t("players.badge.online");
    if (p.last_seen === null) return "—";
    const days = Math.floor((Date.now() / 1000 - p.last_seen) / 86400);
    if (days <= 0) return t("players.seenToday");
    if (days < 60) return t("players.seenDaysAgo").replace("{n}", String(days));
    return t("players.seenMonthsAgo").replace("{n}", String(Math.floor(days / 30)));
  }

  $effect(() => {
    if (!active) return;
    refresh();
    if (playerData && !loadedOnce) {
      loadedOnce = true;
      // Hors suivi : la recherche et le tri ne doivent pas relancer cet effet.
      untrack(() => void loadKnown(false));
    }
    const id = setInterval(refresh, 10000);
    return () => clearInterval(id);
  });
</script>

<div class="view">
  {#if error}<p class="err selectable">{error}</p>{/if}
  {#if notice}<p class="ok">{notice}</p>{/if}

  <!-- Joueurs en ligne -->
  <section>
    <h3>
      {t("players.online")}
      {#if live?.players}<span class="dim">· {live.players.current}/{live.players.limit}</span>{/if}
    </h3>

    {#if live?.players && live.players.list.length > 0}
      <ul>
        {#each live.players.list as name (name)}
          <li class="player">
            <span class="pname selectable">{name}</span>
            <div class="acts">
              <button disabled={busy !== null} onclick={() => act(name, "kick")}>{t("action.kick")}</button>
              <button class="danger" disabled={busy !== null} onclick={() => act(name, "ban")}>{t("action.ban")}</button>
              <button disabled={busy !== null} onclick={() => act(name, "op_add")}>{t("action.op")}</button>
            </div>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="dim">{t("players.none")}</p>
    {/if}
  </section>

  <!-- Action sur un pseudo arbitraire -->
  <section>
    <h3>{t("players.byName")}</h3>
    <input
      class="selectable"
      type="text"
      autocapitalize="off"
      autocomplete="off"
      spellcheck="false"
      placeholder={t("players.pseudo")}
      bind:value={pseudo}
    />
    <div class="grid">
      {#each byNameActions as b (b.action)}
        <button
          class:danger={b.action === "ban"}
          disabled={busy !== null || pseudo.trim() === ""}
          onclick={() => act(pseudo, b.action)}
        >
          {t(b.key)}
        </button>
      {/each}
    </div>
  </section>

  <!-- Joueurs déjà venus (Minecraft Java) -->
  {#if playerData}
    <section>
      <h3>{t("players.all")}</h3>
      <p class="dim small">{t("players.allHint")}</p>

      {#if known && known.stats.total > 0}
        <div class="tiles">
          <div class="tile"><b>{known.stats.total}</b><span>{t("players.stats.unique")}</span></div>
          <div class="tile"><b>{known.stats.active_7d}</b><span>{t("players.stats.active7d")}</span></div>
          <div class="tile"><b>{playTime(known.stats.total_play_time_secs)}</b><span>{t("players.stats.totalTime")}</span></div>
        </div>
      {/if}
      {#if known && !known.playerdata_found}<p class="warn">{t("players.noPlayerdata")}</p>{/if}
      {#if known?.other_worlds}<p class="warn">{t("players.otherWorlds")}</p>{/if}

      <div class="filters">
        <input
          class="selectable"
          type="search"
          autocapitalize="off"
          autocomplete="off"
          spellcheck="false"
          placeholder={t("players.search")}
          value={search}
          oninput={(e) => onSearch(e.currentTarget.value)}
        />
        <select value={sort} onchange={(e) => onSort(e.currentTarget.value as PlayerSort)}>
          {#each SORTS as s (s)}<option value={s}>{t(`players.sort.${s}`)}</option>{/each}
        </select>
      </div>

      {#if knownError}
        <p class="err selectable">{knownError}</p>
      {:else if known && knownList.length === 0}
        <p class="dim">{known.stats.total === 0 ? t("players.empty") : t("players.noMatch")}</p>
      {:else}
        <ul>
          {#each knownList as p (p.uuid)}
            <li>
              <button class="known" onclick={() => (sheet = p)}>
                <span class="kname">
                  <span class="pname" class:unknown={!p.name}>{p.name ?? t("players.unknownName")}</span>
                  {#if p.flags.op}<span class="tag op">{t("players.badge.op")}</span>{/if}
                  {#if p.flags.whitelisted}<span class="tag wl">{t("players.badge.whitelisted")}</span>{/if}
                  {#if p.flags.banned}<span class="tag ban">{t("players.badge.banned")}</span>{/if}
                </span>
                <span class="kmeta dim">
                  <span>{playTime(p.play_time_secs)}</span>
                  <span class:on={p.online}>{lastSeen(p)}</span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
        {#if known}
          <p class="dim small center">{t("players.count").replace("{total}", String(known.total))}</p>
          {#if knownList.length < known.total}
            <button class="more" disabled={knownLoading} onclick={() => loadKnown(true)}>{t("players.loadMore")}</button>
          {/if}
        {:else if knownLoading}
          <p class="dim center">…</p>
        {/if}
      {/if}
    </section>
  {/if}
</div>

{#if sheet}
  <PlayerSheet {serverId} player={sheet} onClose={() => (sheet = null)} />
{/if}

<style>
  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 14px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .player {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
  }
  .pname {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pname.unknown {
    font-style: italic;
    color: var(--text-dim);
  }
  .acts {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .acts button {
    padding: 8px 10px;
    font-size: 13px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--text);
    min-height: 38px;
  }
  input,
  select {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    padding: 12px;
    font-size: 16px;
    outline: none;
  }
  input:focus,
  select:focus {
    border-color: var(--brand-primary);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .grid button,
  .more {
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    font-weight: 600;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .tile b {
    font-size: 17px;
  }
  .tile span {
    font-size: 12px;
    color: var(--text-dim);
  }
  .filters {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 8px;
  }
  .filters input {
    min-width: 0;
  }
  .known {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 4px;
    text-align: left;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    color: var(--text);
    font: inherit;
  }
  .kname {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .kmeta {
    display: flex;
    justify-content: space-between;
    font-size: 13px;
  }
  .kmeta .on {
    color: var(--brand-primary);
  }
  .tag {
    flex: none;
    font-size: 11px;
    font-weight: 700;
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid currentColor;
  }
  .tag.op {
    color: var(--brand-primary);
  }
  .tag.wl {
    color: var(--state-pending);
  }
  .tag.ban {
    color: var(--state-danger);
  }
  button.danger {
    color: var(--state-danger);
  }
  button:disabled {
    opacity: 0.45;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
  }
  .warn {
    margin: 0;
    font-size: 13px;
    color: var(--state-pending);
  }
  .ok {
    margin: 0;
    color: var(--brand-primary);
    font-size: 14px;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 13px;
    margin: 0;
  }
  .center {
    text-align: center;
    margin: 0;
  }
</style>
