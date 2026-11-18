<script lang="ts">
  /**
   * Disponibilité d'un serveur (aperçu) : pourcentage sur 24 h / 7 j / 30 j et journal des
   * coupures, calculés par le cœur à partir de la surveillance de l'app ou de son service
   * (crates/minestrator-core/src/uptime.rs). Les périodes non surveillées ne comptent pas.
   */
  import { onMount, onDestroy } from 'svelte';
  import { api } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { fmtDate, fmtDateTime, fmtDuration } from '#lib/automations/format.ts';
  import type { UptimeReport } from '#lib/types.ts';

  let { serverId }: { serverId: number } = $props();

  let report = $state<UptimeReport | null>(null);
  let showAll = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  const PREVIEW = 4;
  const outages = $derived(report ? (showAll ? report.outages : report.outages.slice(0, PREVIEW)) : []);

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
    if (pct >= 99.5) return 'var(--state-running)';
    if (pct >= 95) return 'var(--state-pending)';
    return 'var(--state-danger)';
  }

  function fmtPct(pct: number): string {
    return pct >= 99.995 ? '100' : pct.toFixed(pct >= 99 ? 2 : 1);
  }

  function causeColor(cause: string): string {
    if (cause === 'crash') return 'var(--state-danger)';
    if (cause === 'stop') return 'var(--state-offline)';
    return 'var(--state-pending)';
  }
</script>

<section class="block">
  <h2 title={t('uptime.hint')}>{t('uptime.title')}</h2>
  <div class="tiles">
    {#each report?.windows ?? [] as w (w.seconds)}
      <div class="card tile">
        <span class="k">{t(`uptime.window.${w.seconds}`)}</span>
        {#if w.percent != null}
          <span class="v" style="color: {color(w.percent)}">{fmtPct(w.percent)} %</span>
          <span class="s dim">
            {t('uptime.monitored', { pct: Math.round((w.monitored_s / w.seconds) * 100) })}
          </span>
        {:else}
          <span class="v dim">—</span>
          <span class="s dim">{t('uptime.noData')}</span>
        {/if}
      </div>
    {/each}
  </div>
  <p class="dim hint">
    {t('uptime.hint')}
    {#if report?.since != null}· {t('uptime.since', { date: fmtDate(report.since) })}{/if}
  </p>

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
      <p class="dim empty">{t('uptime.noOutage')}</p>
    {:else}
      <div class="outs">
        {#each outages as o (o.start)}
          <div class="out">
            <span class="cause" style="color: {causeColor(o.cause)}">{t(`uptime.cause.${o.cause}`)}</span>
            <span class="when dim">{fmtDateTime(o.start)}</span>
            <span class="dur">{fmtDuration(o.duration_s)}</span>
            <span class="end dim">
              {#if o.end == null}
                {t('uptime.ongoing')}
              {:else if !o.end_known}
                {t('uptime.endUnknown')}
              {/if}
            </span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .block {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
  }
  .tile {
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .k {
    font-size: 12px;
    color: var(--text-muted);
    font-weight: 600;
  }
  .v {
    font-size: 22px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.01em;
  }
  .s {
    font-size: 11.5px;
  }
  .hint {
    margin: 0;
    font-size: 12px;
  }
  .out-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 4px;
  }
  .out-title {
    font-size: 13px;
    font-weight: 600;
  }
  .empty {
    font-size: 12.5px;
    font-style: italic;
    margin: 0;
  }
  .outs {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .out {
    display: grid;
    grid-template-columns: 110px 150px 90px 1fr;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 13px;
  }
  .out:nth-child(odd) {
    background: color-mix(in srgb, var(--text) 3%, transparent);
  }
  .cause {
    font-weight: 600;
  }
  .when,
  .dur {
    font-variant-numeric: tabular-nums;
  }
  .end {
    font-size: 12px;
  }
  .linkbtn {
    background: none;
    border: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 0;
  }
</style>
