<script lang="ts">
  /**
   * Disponibilité d'un serveur (aperçu) : pourcentage sur 24 h / 7 j / 30 j et journal des
   * coupures, calculés par le cœur à partir de la surveillance de l'app ou de son service,
   * complétée par les minutes mesurées de l'hébergeur là où rien ne surveillait
   * (crates/minestrator-core/src/uptime.rs). Elles prouvent que le serveur tournait, jamais une
   * coupure : leur part est indiquée. Les périodes sans aucune mesure ne comptent pas.
   */
  import { onMount, onDestroy } from 'svelte';
  import { api } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { fmtDate, fmtDateTime, fmtDuration } from '#lib/automations/format.ts';
  import type { UptimeReport, UptimeWindow } from '#lib/types.ts';
  import Icon from './Icon.svelte';

  let { serverId }: { serverId: number } = $props();

  let report = $state<UptimeReport | null>(null);
  let showAll = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  const PREVIEW = 4;
  const outages = $derived(report ? (showAll ? report.outages : report.outages.slice(0, PREVIEW)) : []);
  /** Fenêtre la plus large qui a des mesures : son taux de surveillance résume la carte. */
  const widest = $derived(
    [...(report?.windows ?? [])].reverse().find((w) => w.percent != null) ?? null
  );

  onMount(() => {
    void load();
    timer = setInterval(() => void load(), 5 * 60 * 1000);
  });
  onDestroy(() => clearInterval(timer));

  async function load() {
    try {
      report = await api.uptimeReport(serverId);
    } catch {
      /* historique indisponible : la carte reste vide */
    }
  }

  /** Couleur d'un pourcentage : vert ≥ 99,5 %, ambre ≥ 95 %, rouge sinon (charte : jauges). */
  function color(pct: number): string {
    if (pct >= 99.5) return 'var(--state-running-text)';
    if (pct >= 95) return 'var(--state-pending)';
    return 'var(--state-danger)';
  }

  function fmtPct(pct: number): string {
    return pct >= 99.995 ? '100' : pct.toFixed(pct >= 99 ? 2 : 1);
  }

  function causeColor(cause: string): string {
    if (cause === 'crash') return 'var(--state-danger)';
    if (cause === 'stop') return 'var(--text-muted)';
    return 'var(--state-pending)';
  }

  /** Part surveillée d'une fenêtre (et part mesurée par l'hébergeur), en une phrase. */
  function coverage(w: UptimeWindow): string {
    const parts = [t('uptime.monitored', { pct: Math.round((w.monitored_s / w.seconds) * 100) })];
    if (w.host_s > 0) parts.push(t('uptime.viaHost', { pct: Math.round((w.host_s / w.seconds) * 100) }));
    return parts.join(', ');
  }
</script>

<section class="card up">
  <h2>
    {t('uptime.title')}
    <span class="info" title={t('uptime.hint')}><Icon name="info" size={14} /></span>
  </h2>
  <div class="tiles">
    {#each report?.windows ?? [] as w (w.seconds)}
      <div class="tile" title={w.percent != null ? coverage(w) : t('uptime.noData')}>
        <span class="k">{t(`uptime.window.${w.seconds}`)}</span>
        {#if w.percent != null}
          <span class="v" style="color: {color(w.percent)}">{fmtPct(w.percent)} %</span>
        {:else}
          <span class="v dim">—</span>
        {/if}
      </div>
    {/each}
  </div>
  {#if widest}
    <p class="sum">
      <span title={t('uptime.viaHostHint')}>{t(`uptime.window.${widest.seconds}`)} : {coverage(widest)}.</span>
      {#if report?.since != null}{t('uptime.since', { date: fmtDate(report.since) })}.{/if}
    </p>
  {:else if report}
    <p class="sum">{t('uptime.noData')}</p>
  {/if}

  {#if report}
    <div class="out-head">
      <span class="out-title">{t('uptime.outages')}</span>
      {#if report.outages.length > PREVIEW}
        <button class="linkbtn" onclick={() => (showAll = !showAll)}>
          {showAll ? t('uptime.showLess') : t('uptime.showAll', { n: report.outages.length })}
        </button>
      {/if}
    </div>
    {#if report.outages.length === 0}
      <p class="empty">{t('uptime.noOutage')}</p>
    {:else}
      <div class="outs">
        {#each outages as o (o.start)}
          <div class="out">
            <span class="cause" style="color: {causeColor(o.cause)}">{t(`uptime.cause.${o.cause}`)}</span>
            <span class="dur">{fmtDuration(o.duration_s)}</span>
            <span class="when">
              {fmtDateTime(o.start)}
              {#if o.end == null}
                · {t('uptime.ongoing')}
              {:else if !o.end_known}
                · {t('uptime.endUnknown')}
              {/if}
            </span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .up {
    padding: 20px 22px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    font-size: 18px;
  }
  .info {
    display: inline-flex;
    color: var(--text-dim);
    cursor: help;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    cursor: default;
  }
  .k {
    font-size: 13px;
    color: var(--text-muted);
    font-weight: 700;
  }
  .v {
    font-size: 22px;
    font-weight: 900;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .sum {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .out-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  .out-title {
    font-size: 14px;
    font-weight: 800;
  }
  .empty {
    font-size: 13px;
    color: var(--text-dim);
    margin: 0;
  }
  .outs {
    display: flex;
    flex-direction: column;
  }
  .out {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0 10px;
    padding: 7px 0;
    font-size: 13.5px;
  }
  .out + .out {
    border-top: 1px solid var(--border);
  }
  .cause {
    font-weight: 800;
  }
  .dur {
    font-family: var(--font-mono);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }
  .when {
    grid-column: 1 / -1;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .linkbtn {
    background: none;
    border: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    font-weight: 800;
    padding: 0;
  }
  .linkbtn:hover {
    text-decoration: underline;
  }
</style>
