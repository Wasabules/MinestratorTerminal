<script lang="ts">
  /**
   * Carte « Historique » de l'Aperçu : CPU / RAM / disque sur 1 h, 6 h ou 24 h.
   *
   * L'API ne fournit aucun historique : ces points viennent du SQLite local du core, alimenté par
   * le superviseur (un échantillon toutes les 10 s par serveur surveillé). Ils n'existent donc que
   * pour les périodes où l'app tournait — au premier plan, ou en continu avec la surveillance en
   * arrière-plan. Lecture locale, sans réseau : on peut rafraîchir souvent sans coût.
   */
  import { api } from "../ipc";
  import { t } from "../i18n";
  import MetricsChart from "./MetricsChart.svelte";
  import type { MetricSample } from "../types";

  let {
    serverId,
    active,
    cpuLimit,
    diskLimit,
  }: { serverId: number; active: boolean; cpuLimit: number; diskLimit: number } = $props();

  const RANGES = [
    { secs: 3600, label: "1 h" },
    { secs: 6 * 3600, label: "6 h" },
    { secs: 24 * 3600, label: "24 h" },
  ];
  const RANGE_KEY = "mnstr-metrics-range";

  let range = $state(readRange());
  let samples = $state<MetricSample[]>([]);
  let now = $state(Math.floor(Date.now() / 1000));

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
      const list = await api.metricsHistory(id, secs);
      if (id !== serverId || secs !== range) return; // réponse périmée
      samples = list;
      now = Math.floor(Date.now() / 1000);
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

<section class="card">
  <header>
    <h3>{t("metrics.title")}</h3>
    <div class="ranges" role="group" aria-label={t("metrics.title")}>
      {#each RANGES as r (r.secs)}
        <button class="rg" class:on={range === r.secs} onclick={() => setRange(r.secs)}>
          {r.label}
        </button>
      {/each}
    </div>
  </header>
  <MetricsChart {samples} {range} {now} {cpuLimit} {diskLimit} />
</section>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .ranges {
    display: inline-flex;
    padding: 2px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg);
  }
  /* Cible tactile ≥ 44 px de haut via le min-height global des boutons ; largeur resserrée. */
  .rg {
    min-width: 48px;
    padding: 0 10px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: var(--text-dim);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
  .rg.on {
    background: color-mix(in srgb, var(--brand-primary) 18%, transparent);
    color: var(--text);
    font-weight: 600;
  }
</style>
