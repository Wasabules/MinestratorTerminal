<script lang="ts">
  /**
   * Historique CPU / RAM d'un serveur, en % de leurs limites, sur une fenêtre fixe
   * (`history.from` → `history.to`).
   *
   * Les points viennent de la fusion faite par le cœur (crates/minestrator-core/src/history.rs) :
   * la surveillance de l'app (`source: local`, trait plein) et, là où elle n'a rien vu, les
   * statistiques à la minute de l'hébergeur (`host`, trait pointillé). Un écart nettement plus
   * grand que le pas des points interrompt la courbe : une période sans aucune mesure reste un
   * trou au lieu d'être masquée par une ligne tirée entre deux points éloignés.
   */
  import { t } from '#lib/i18n/index.ts';
  import type { HistoryPoint, MetricsHistory } from '#lib/types.ts';

  let { history, cpuLimit }: { history: MetricsHistory | null; cpuLimit: number } = $props();

  const H = 170;
  const PAD_L = 30;
  const PAD_R = 10;
  const PAD_T = 10;
  const PAD_B = 22;

  let w = $state(600);

  const chartW = $derived(Math.max(1, w - PAD_L - PAD_R));
  const chartH = H - PAD_T - PAD_B;
  const samples = $derived(history?.points ?? []);
  const t0 = $derived(history?.from ?? 0);
  const t1 = $derived(history && history.to > history.from ? history.to : t0 + 1);
  /** Au-delà, deux points voisins ne sont pas reliés (trou de données). */
  const gapS = $derived(Math.max(3 * (history?.step_s ?? 10), 150));
  const hasHost = $derived(samples.some((s) => s.source === 'host'));

  function clamp(v: number): number {
    return Math.max(0, Math.min(100, v));
  }
  function cpuPct(s: HistoryPoint): number {
    return cpuLimit > 0 ? (s.cpu / cpuLimit) * 100 : 0;
  }
  function ramPct(s: HistoryPoint): number {
    return s.mem_limit > 0 ? (s.mem / s.mem_limit) * 100 : 0;
  }

  function x(ts: number): number {
    return PAD_L + ((ts - t0) / (t1 - t0)) * chartW;
  }
  function y(pct: number): number {
    return PAD_T + (1 - clamp(pct) / 100) * chartH;
  }

  type Segment = { host: boolean; d: string };

  /**
   * Tracés par segment continu et par origine : un changement d'origine sans trou repart du point
   * précédent (la courbe reste continue), un trou ouvre un nouveau segment.
   */
  function segments(fn: (s: HistoryPoint) => number): Segment[] {
    const out: { host: boolean; pts: HistoryPoint[] }[] = [];
    let prev: HistoryPoint | null = null;
    for (const s of samples) {
      const host = s.source === 'host';
      const cur = out[out.length - 1];
      if (!cur || !prev || s.ts - prev.ts > gapS) {
        out.push({ host, pts: [s] });
      } else if (cur.host !== host) {
        out.push({ host, pts: [prev, s] });
      } else {
        cur.pts.push(s);
      }
      prev = s;
    }
    return out.map((seg) => ({
      host: seg.host,
      d:
        seg.pts.map((s, i) => `${i ? 'L' : 'M'}${x(s.ts).toFixed(1)} ${y(fn(s)).toFixed(1)}`).join(' ') +
        (seg.pts.length === 1 ? ' l0.1 0' : ''),
    }));
  }

  const cpuSegs = $derived(segments(cpuPct));
  const ramSegs = $derived(segments(ramPct));
  const last = $derived(samples.length ? samples[samples.length - 1] : null);
  const gridY = [0, 50, 100];
  const xTicks = $derived([t0, (t0 + t1) / 2, t1]);

  // --- Survol : point le plus proche du curseur + infobulle ---
  let hoverIdx = $state<number | null>(null);
  const hover = $derived(hoverIdx != null && hoverIdx < samples.length ? samples[hoverIdx] : null);

  function nearestIndex(ts: number): number {
    // Recherche binaire (points triés par ts croissant).
    let lo = 0;
    let hi = samples.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (samples[mid].ts < ts) lo = mid + 1;
      else hi = mid;
    }
    if (lo > 0 && Math.abs(samples[lo - 1].ts - ts) <= Math.abs(samples[lo].ts - ts)) return lo - 1;
    return lo;
  }
  function onMove(e: MouseEvent) {
    if (samples.length === 0) return;
    const rect = (e.currentTarget as Element).getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left - PAD_L) / chartW));
    hoverIdx = nearestIndex(t0 + frac * (t1 - t0));
  }
  function onLeave() {
    hoverIdx = null;
  }
  const long = $derived(t1 - t0 > 86400);
  function fmtTime(ts: number): string {
    const d = new Date(ts * 1000);
    return long
      ? d.toLocaleString(undefined, { day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit' })
      : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }
  function fmtTick(ts: number): string {
    const d = new Date(ts * 1000);
    return long
      ? d.toLocaleDateString(undefined, { day: '2-digit', month: 'short' })
      : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }
</script>

<div class="chart" bind:clientWidth={w}>
  {#if samples.length === 0}
    <div class="empty dim">{t('overview.noHistory')}</div>
  {:else}
    <svg
      width={w}
      height={H}
      role="img"
      aria-label="{t('overview.history')} CPU / RAM"
      onmousemove={onMove}
      onmouseleave={onLeave}
    >
      {#each gridY as g (g)}
        <line class="grid" x1={PAD_L} x2={w - PAD_R} y1={y(g)} y2={y(g)} />
        <text class="axis" x={PAD_L - 6} y={y(g) + 3} text-anchor="end">{g}</text>
      {/each}
      {#each xTicks as tk, i (i)}
        <text
          class="axis"
          x={x(tk)}
          y={H - 6}
          text-anchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}
        >{fmtTick(tk)}</text>
      {/each}
      {#each cpuSegs as s, i (i)}
        <path class="serie cpu" class:host={s.host} d={s.d} />
      {/each}
      {#each ramSegs as s, i (i)}
        <path class="serie ram" class:host={s.host} d={s.d} />
      {/each}
      {#if hover}
        <line class="cross" x1={x(hover.ts)} x2={x(hover.ts)} y1={PAD_T} y2={PAD_T + chartH} />
        <circle class="dot cpu" cx={x(hover.ts)} cy={y(cpuPct(hover))} r="3.5" />
        <circle class="dot ram" cx={x(hover.ts)} cy={y(ramPct(hover))} r="3.5" />
      {:else if last}
        <circle class="dot cpu" cx={x(last.ts)} cy={y(cpuPct(last))} r="3" />
        <circle class="dot ram" cx={x(last.ts)} cy={y(ramPct(last))} r="3" />
      {/if}
    </svg>
    {#if hover}
      <div class="tip" class:flip={x(hover.ts) > w / 2} style="left: {x(hover.ts)}px">
        <div class="tip-time">{fmtTime(hover.ts)}</div>
        <div class="tip-row">
          <span class="sw cpu"></span><span class="lbl">{t('overview.cpu')}</span>
          <b>{cpuPct(hover).toFixed(0)}%</b>
        </div>
        <div class="tip-row">
          <span class="sw ram"></span><span class="lbl">{t('overview.ram')}</span>
          <b>{ramPct(hover).toFixed(0)}%</b>
        </div>
        <div class="tip-src">
          {hover.source === 'host' ? t('overview.sourceHost') : t('overview.sourceLocal')}
        </div>
      </div>
    {/if}
  {/if}

  <div class="legend">
    <span class="item"><span class="sw cpu"></span>{t('overview.cpu')}
      {#if last}<b>{cpuPct(last).toFixed(0)}%</b>{/if}</span>
    <span class="item"><span class="sw ram"></span>{t('overview.ram')}
      {#if last}<b>{ramPct(last).toFixed(0)}%</b>{/if}</span>
    <span class="srcs">
      {#if hasHost}
        <span class="item src" title={t('overview.sourceHint')}>
          <span class="sw dash"></span>{t('overview.sourceHostShort')}
        </span>
      {/if}
      {#if history && !history.host_ok}
        <span class="item src warn" title={t('overview.hostUnavailableHint')}>
          {t('overview.hostUnavailable')}
        </span>
      {/if}
    </span>
  </div>
</div>

<style>
  .chart {
    width: 100%;
    position: relative;
  }
  .empty {
    height: 170px;
    display: grid;
    place-items: center;
    font-size: 13px;
    text-align: center;
    padding: 0 20px;
  }
  svg {
    display: block;
    cursor: crosshair;
  }
  .cross {
    stroke: var(--text-dim);
    stroke-width: 1;
    stroke-dasharray: 3 3;
    pointer-events: none;
  }
  .tip {
    position: absolute;
    top: 4px;
    transform: translateX(9px);
    pointer-events: none;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: 7px;
    box-shadow: var(--shadow);
    padding: 6px 9px;
    font-size: 11.5px;
    white-space: nowrap;
    z-index: 5;
  }
  .tip.flip {
    transform: translateX(calc(-100% - 9px));
  }
  .tip-time {
    color: var(--text-dim);
    font-family: var(--font-mono);
    font-size: 10.5px;
    margin-bottom: 4px;
  }
  .tip-row {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .tip-row .lbl {
    color: var(--text-muted);
  }
  .tip-row b {
    color: var(--text);
    font-family: var(--font-mono);
    margin-left: auto;
    padding-left: 12px;
  }
  .tip-src {
    margin-top: 4px;
    color: var(--text-dim);
    font-size: 10.5px;
  }
  .grid {
    stroke: var(--border-strong);
    stroke-width: 1;
  }
  .axis {
    fill: var(--text-dim);
    font-size: 10px;
    font-family: var(--font-mono);
  }
  .serie {
    fill: none;
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  /* Mesures de l'hébergeur : pointillé discret, même couleur. */
  .serie.host {
    stroke-dasharray: 5 4;
    stroke-linecap: butt;
    opacity: 0.85;
  }
  .serie.cpu,
  .dot.cpu {
    stroke: var(--chart-cpu);
  }
  .serie.ram,
  .dot.ram {
    stroke: var(--chart-ram);
  }
  .dot {
    fill: var(--surface);
    stroke-width: 2;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    margin-top: 8px;
    padding-left: 30px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .item {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .item b {
    color: var(--text);
    font-family: var(--font-mono);
    font-weight: 500;
  }
  .srcs {
    display: inline-flex;
    gap: 14px;
    margin-left: auto;
  }
  .item.src {
    color: var(--text-dim);
    cursor: help;
  }
  .item.warn {
    color: var(--state-pending);
  }
  .sw {
    width: 14px;
    height: 3px;
    border-radius: 2px;
    display: inline-block;
  }
  .sw.cpu {
    background: var(--chart-cpu);
  }
  .sw.ram {
    background: var(--chart-ram);
  }
  .sw.dash {
    height: 0;
    border-radius: 0;
    border-top: 2px dashed var(--text-dim);
  }
</style>
