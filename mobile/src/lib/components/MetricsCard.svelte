<script lang="ts">
  /**
   * Carte « Historique » de l'Aperçu : CPU / RAM / disque sur 1 h, 6 h, 24 h ou 7 jours, et pic de
   * joueurs connectés.
   *
   * Le cœur croise deux sources (crates/minestrator-core/src/history.rs) : le SQLite local, alimenté
   * par le superviseur (un échantillon toutes les 10 s pendant que l'app surveille), et les
   * statistiques à la minute de l'hébergeur, qui comblent les périodes où l'app ne tournait pas. Le
   * cœur garde ces dernières en cache (seule la tranche en cours est relue, au plus une fois par
   * minute) : rafraîchir toutes les 30 s ne coûte presque rien.
   */
  import { api } from "../ipc";
  import { t } from "../i18n";
  import MetricsChart from "./MetricsChart.svelte";
  import type { MetricsHistory } from "../types";

  let {
    serverId,
    active,
    cpuLimit,
    diskLimit,
  }: { serverId: number; active: boolean; cpuLimit: number; diskLimit: number } = $props();

  /** Libellé, ou clé i18n traduite au rendu. */
  const RANGES: { secs: number; label?: string; key?: string }[] = [
    { secs: 3600, label: "1 h" },
    { secs: 6 * 3600, label: "6 h" },
    { secs: 24 * 3600, label: "24 h" },
    { secs: 7 * 24 * 3600, key: "metrics.range7d" },
  ];
  /** Points demandés au cœur (le graphe en affiche au plus 240). */
  const MAX_POINTS = 480;
  const RANGE_KEY = "mnstr-metrics-range";

  let range = $state(readRange());
  let history = $state<MetricsHistory | null>(null);
  const now = $derived(history?.to ?? Math.floor(Date.now() / 1000));
  const peak = $derived(history?.players.reduce((m, p) => Math.max(m, p.count), 0) ?? 0);
  const lastPlayers = $derived(history?.players.length ? history.players[history.players.length - 1] : null);

  /** Préférence d'affichage (pas une donnée sensible) : la fenêtre choisie est retenue. */
  function readRange(): number {
    try {
      const v = Number(localStorage.getItem(RANGE_KEY));
      return RANGES.some((r) => r.secs === v) ? v : 24 * 3600;
    } catch {
      return 24 * 3600;
    }
  }

  function setRange(secs: number) {
    range = secs;
    try {
      localStorage.setItem(RANGE_KEY, String(secs));
    } catch {
      /* stockage indisponible : sans conséquence */
    }
    // Rechargement assuré par l'effet ci-dessous, qui dépend de `range`.
  }

  async function load() {
    const id = serverId;
    const secs = range;
    try {
      const h = await api.metricsHistory(id, secs, MAX_POINTS);
      if (id !== serverId || secs !== range) return; // réponse périmée
      history = h;
    } catch {
      /* historique indisponible : la carte affiche son état vide */
    }
  }

  // Rafraîchi quand l'Aperçu est affiché et à chaque changement de fenêtre (dépendance
  // explicite à `range`).
  $effect(() => {
    if (!active) return;
    void range;
    void load();
    const iv = setInterval(load, 30_000);
    return () => clearInterval(iv);
  });
</script>

<section class="card hist">
  <header>
    <h2>{t("metrics.title")}</h2>
    <!-- Période : la sélection prend l'accent du biome. -->
    <div class="ranges" role="group" aria-label={t("metrics.title")}>
      {#each RANGES as r (r.secs)}
        <button
          class="rg"
          class:on={range === r.secs}
          aria-pressed={range === r.secs}
          onclick={() => setRange(r.secs)}
        >
          {r.key ? t(r.key) : r.label}
        </button>
      {/each}
    </div>
  </header>
  <MetricsChart
    samples={history?.points ?? []}
    stepS={history?.step_s ?? 10}
    {range}
    {now}
    {cpuLimit}
    {diskLimit}
  />
  {#if history && history.players.length > 0}
    <p class="players">
      {t("metrics.players")}
      <b>{t("metrics.playersPeak").replace("{n}", String(peak))}</b>
      {#if lastPlayers}
        <span class="dim">· {t("metrics.playersLast").replace("{n}", String(lastPlayers.count))}</span>
      {/if}
    </p>
  {/if}
  {#if history && history.host_s > 0}
    <p class="src">
      <span class="dash" aria-hidden="true"></span>
      {t("metrics.hostHint")}
    </p>
  {:else if history && !history.host_ok}
    <p class="src">{t("metrics.hostUnavailable")}</p>
  {/if}
</section>

<style>
  .hist {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
  }
  header {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
  }
  /* Segment de période sur toute la largeur : quatre cibles larges, au pouce. */
  .ranges {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 2px;
    padding: 3px;
    border-radius: 12px;
    background: var(--bg);
  }
  .rg {
    min-height: 40px;
    padding: 0 6px;
    border: none;
    border-radius: 9px;
    background: transparent;
    color: var(--text-muted);
    font-size: 14px;
    font-weight: 800;
    font-variant-numeric: tabular-nums;
  }
  .rg.on {
    background: var(--accent);
    color: var(--on-accent);
  }
  .players {
    margin: 0;
    padding-top: 10px;
    border-top: 1px solid var(--border-strong);
    font-size: 13.5px;
    color: var(--text-muted);
  }
  .players b {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .src {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-dim);
  }
  /* Rappel du trait pointillé des mesures de l'hébergeur. */
  .dash {
    flex: none;
    width: 18px;
    border-top: 2px dashed var(--text-dim);
  }
  .dim {
    color: var(--text-dim);
  }
</style>
