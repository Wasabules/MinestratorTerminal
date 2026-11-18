<script lang="ts">
  /**
   * Graphique d'historique CPU / RAM / disque, en SVG maison (aucune dépendance).
   *
   * Les points viennent de la fusion du cœur (crates/minestrator-core/src/history.rs) : la
   * surveillance de l'app (trait plein) et, là où elle n'a rien vu, les mesures à la minute de
   * l'hébergeur (`source: host`, trait pointillé).
   *
   * Un seul axe : les trois séries sont exprimées en pourcentage de leur limite, ce qui permet de
   * les superposer sans double échelle. La fenêtre est fixe (`range` secondes jusqu'à `now`) :
   * une période sans échantillon — app fermée, surveillance coupée — reste visible comme un trou,
   * au lieu d'être masquée par une ligne tirée entre deux points éloignés.
   *
   * Pensé pour le tactile : le doigt balaie le graphique (repère vertical + valeurs dans la
   * légende, au-dessus du doigt et non dessous). `data-noswipe` empêche ce geste de changer
   * d'onglet dans le pager du serveur.
   *
   * Palette (DESIGN.md + vérification daltonisme) : CPU = vert de la marque, RAM = bleu,
   * disque = orange, un pas propre à chaque thème. Le couple vert/corail de la charte se confond
   * en protanopie, d'où le bleu et l'orange.
   */
  import { t } from "../i18n";
  import type { HistoryPoint } from "../types";

  let {
    samples,
    stepS,
    range,
    now,
    cpuLimit,
    diskLimit,
  }: {
    samples: HistoryPoint[];
    /** Pas des points fournis par le cœur (s) : base du seuil de trou. */
    stepS: number;
    /** Largeur de la fenêtre, en secondes. */
    range: number;
    /** Fin de la fenêtre (secondes Unix). */
    now: number;
    /** Limite CPU en % absolu (100 = un cœur) ; ≤ 0 : série à zéro. */
    cpuLimit: number;
    /** Limite disque en octets ; ≤ 0 : série masquée. */
    diskLimit: number;
  } = $props();

  const H = 168;
  const PAD_L = 34;
  const PAD_R = 10;
  const PAD_T = 8;
  const PAD_B = 22;

  let w = $state(320);
  const chartW = $derived(Math.max(1, w - PAD_L - PAD_R));
  const chartH = H - PAD_T - PAD_B;
  const t0 = $derived(now - range);

  /** `host` : la tranche ne contient que des mesures de l'hébergeur. */
  type Point = { ts: number; cpu: number; ram: number; disk: number | null; host: boolean };
  type SeriesKey = "cpu" | "ram" | "disk";


  /**
   * Regroupement par tranches de temps (≈ un point tous les 2 px) : 24 h d'échantillons toutes
   * les 10 s donnent 8 640 points, bien plus que l'écran n'en affiche. Chaque tranche garde le
   * **maximum** de chaque série : un pic de charge ne doit pas disparaître dans une moyenne.
   */
  const buckets = $derived(Math.max(30, Math.min(240, Math.floor(chartW / 2))));
  const bucketSize = $derived(range / buckets);

  const points = $derived.by(() => {
    const acc = new Map<number, Point>();
    for (const s of samples) {
      if (s.ts < t0) continue;
      const b = Math.min(buckets - 1, Math.max(0, Math.floor((s.ts - t0) / bucketSize)));
      const cpu = cpuLimit > 0 ? (s.cpu / cpuLimit) * 100 : 0;
      const ram = s.mem_limit > 0 ? (s.mem / s.mem_limit) * 100 : 0;
      const disk = diskLimit > 0 ? (s.disk / diskLimit) * 100 : null;
      const host = s.source === "host";
      const p = acc.get(b);
      if (!p) {
        acc.set(b, { ts: s.ts, cpu, ram, disk, host });
      } else {
        p.ts = Math.max(p.ts, s.ts);
        p.cpu = Math.max(p.cpu, cpu);
        p.ram = Math.max(p.ram, ram);
        p.disk = disk === null || p.disk === null ? p.disk ?? disk : Math.max(p.disk, disk);
        p.host = p.host && host;
      }
    }
    return [...acc.entries()].sort((a, b) => a[0] - b[0]).map(([, p]) => p);
  });

  /** Au-delà de cet écart entre deux points, la ligne s'interrompt (période sans données). */
  const gapS = $derived(Math.max(3 * bucketSize, 3 * stepS, 150));

  /**
   * Haut de l'axe : 100 %, sauf si une série le dépasse (CPU en rafale au-delà de la limite) —
   * l'axe s'étend alors par pas de 50 % plutôt que d'écrêter un pic que la légende annonce.
   */
  const yMax = $derived.by(() => {
    let m = 100;
    for (const p of points) m = Math.max(m, p.cpu, p.ram, p.disk ?? 0);
    return Math.min(400, Math.ceil(m / 50) * 50);
  });
  const yTicks = $derived(yMax > 100 ? [0, 50, 100, yMax] : [0, 50, 100]);

  function x(ts: number): number {
    return PAD_L + ((ts - t0) / range) * chartW;
  }
  function y(pct: number): number {
    return PAD_T + (1 - Math.max(0, Math.min(yMax, pct)) / yMax) * chartH;
  }

  /**
   * Tracés d'une série, séparés par origine (`local` plein, `host` pointillé). Un trou ou une
   * valeur absente coupe la ligne ; un changement d'origine sans trou repart du point précédent
   * pour que la courbe reste continue.
   */
  function path(key: SeriesKey): { local: string; host: string } {
    const out = { local: "", host: "" };
    let prev: Point | null = null;
    let segLen = 0;
    let seg: "local" | "host" = "local";
    const close = () => {
      if (segLen === 1) out[seg] += " l0.1 0"; // point isolé : rendu par le bout arrondi
    };
    for (const p of points) {
      const v = p[key];
      if (v === null) {
        prev = null;
        continue;
      }
      const src = p.host ? "host" : "local";
      const pv = prev ? prev[key] : null;
      const gap = !prev || pv === null || p.ts - prev.ts > gapS;
      if (gap || src !== seg) {
        close();
        seg = src;
        if (gap || !prev || pv === null) {
          out[seg] += ` M${x(p.ts).toFixed(1)} ${y(v).toFixed(1)}`;
          segLen = 1;
        } else {
          out[seg] += ` M${x(prev.ts).toFixed(1)} ${y(pv).toFixed(1)} L${x(p.ts).toFixed(1)} ${y(v).toFixed(1)}`;
          segLen = 2;
        }
      } else {
        out[seg] += ` L${x(p.ts).toFixed(1)} ${y(v).toFixed(1)}`;
        segLen += 1;
      }
      prev = p;
    }
    close();
    return { local: out.local.trim(), host: out.host.trim() };
  }

  const hasDisk = $derived(diskLimit > 0 && points.some((p) => p.disk !== null));
  const series = $derived(
    [
      { key: "cpu" as const, label: t("overview.cpu") },
      { key: "ram" as const, label: t("overview.ram") },
      ...(hasDisk ? [{ key: "disk" as const, label: t("overview.disk") }] : []),
    ].map((s) => ({ ...s, d: path(s.key), peak: peak(s.key) })),
  );

  function peak(key: SeriesKey): number | null {
    let m: number | null = null;
    for (const p of points) {
      const v = p[key];
      if (v !== null && (m === null || v > m)) m = v;
    }
    return m;
  }

  const last = $derived(points.length ? points[points.length - 1] : null);

  // --- Balayage au doigt : point le plus proche + valeurs dans la légende ---
  let scrubIdx = $state<number | null>(null);
  const shown = $derived(
    scrubIdx !== null && scrubIdx < points.length ? points[scrubIdx] : last,
  );

  function nearest(ts: number): number {
    let lo = 0;
    let hi = points.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (points[mid].ts < ts) lo = mid + 1;
      else hi = mid;
    }
    if (lo > 0 && Math.abs(points[lo - 1].ts - ts) <= Math.abs(points[lo].ts - ts)) return lo - 1;
    return lo;
  }

  function scrub(e: PointerEvent) {
    if (points.length === 0) return;
    const rect = (e.currentTarget as Element).getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left - PAD_L) / chartW));
    scrubIdx = nearest(t0 + frac * range);
  }
  function onDown(e: PointerEvent) {
    (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
    scrub(e);
  }
  function onMove(e: PointerEvent) {
    if (scrubIdx !== null || e.pointerType === "mouse") scrub(e);
  }
  function release() {
    scrubIdx = null;
  }

  function fmtClock(ts: number): string {
    return new Date(ts * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  }
  /** Au-delà d'une journée, la date compte plus que l'heure. */
  function fmtTick(ts: number): string {
    return range > 86400
      ? new Date(ts * 1000).toLocaleDateString(undefined, { day: "2-digit", month: "short" })
      : fmtClock(ts);
  }
  function fmtWhen(ts: number): string {
    return range > 86400
      ? new Date(ts * 1000).toLocaleString(undefined, {
          day: "2-digit",
          month: "short",
          hour: "2-digit",
          minute: "2-digit",
        })
      : fmtClock(ts);
  }
  function fmtPct(v: number | null | undefined): string {
    return v === null || v === undefined ? "—" : `${Math.round(v)} %`;
  }

  const xTicks = $derived([t0, t0 + range / 2, now]);
</script>

<div class="chart" bind:clientWidth={w}>
  <div class="readout" aria-live="polite">
    <span class="when dim">
      {#if scrubIdx !== null && shown}
        {fmtWhen(shown.ts)}{#if shown.host}&nbsp;· {t("metrics.fromHost")}{/if}
      {:else if last}
        {t("metrics.now")} · {fmtWhen(last.ts)}
      {:else}
        &nbsp;
      {/if}
    </span>
    <div class="legend">
      {#each series as s (s.key)}
        <span class="item">
          <span class="sw {s.key}"></span>
          <span class="lbl">{s.label}</span>
          <b>{fmtPct(shown?.[s.key])}</b>
          {#if s.peak !== null && scrubIdx === null}
            <span class="pk dim">{t("metrics.peak")} {fmtPct(s.peak)}</span>
          {/if}
        </span>
      {/each}
    </div>
  </div>

  {#if points.length === 0}
    <div class="empty dim">{t("metrics.noData")}</div>
  {:else}
    <svg
      data-noswipe
      width={w}
      height={H}
      role="img"
      aria-label={t("metrics.chartLabel")}
      onpointerdown={onDown}
      onpointermove={onMove}
      onpointerup={release}
      onpointercancel={release}
      onpointerleave={release}
    >
      {#each yTicks as g (g)}
        <line class="grid" x1={PAD_L} x2={w - PAD_R} y1={y(g)} y2={y(g)} />
        <text class="axis" x={PAD_L - 6} y={y(g) + 3.5} text-anchor="end">{g}%</text>
      {/each}
      {#each xTicks as tk, i (i)}
        <text
          class="axis"
          x={x(tk)}
          y={H - 6}
          text-anchor={i === 0 ? "start" : i === xTicks.length - 1 ? "end" : "middle"}
        >{fmtTick(tk)}</text>
      {/each}

      {#each series as s (s.key)}
        <path class="serie {s.key}" d={s.d.local} />
        <path class="serie host {s.key}" d={s.d.host} />
      {/each}

      {#if shown}
        {#if scrubIdx !== null}
          <line class="cross" x1={x(shown.ts)} x2={x(shown.ts)} y1={PAD_T} y2={PAD_T + chartH} />
        {/if}
        {#each series as s (s.key)}
          {@const v = shown[s.key]}
          {#if v !== null}
            <circle class="dot {s.key}" cx={x(shown.ts)} cy={y(v)} r="4" />
          {/if}
        {/each}
      {/if}
    </svg>
  {/if}
</div>

<style>
  /* Un pas de couleur par thème, chacun vérifié contre la surface de son mode (contraste ≥ 3:1,
     séparation des trois séries en daltonisme). */
  .chart {
    --viz-cpu: #00a87c;
    --viz-ram: #3987e5;
    --viz-disk: #d95926;
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  :global(:root[data-theme="light"]) .chart {
    --viz-cpu: #009b72;
    --viz-ram: #2a78d6;
    --viz-disk: #eb6834;
  }

  .readout {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-height: 40px;
  }
  .when {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
  }
  .item {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .item b {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .pk {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .sw {
    width: 10px;
    height: 3px;
    border-radius: 2px;
    align-self: center;
  }
  .sw.cpu {
    background: var(--viz-cpu);
  }
  .sw.ram {
    background: var(--viz-ram);
  }
  .sw.disk {
    background: var(--viz-disk);
  }

  svg {
    display: block;
    /* Défilement vertical laissé au navigateur ; le balayage horizontal pilote le repère. */
    touch-action: pan-y;
  }
  .grid {
    stroke: var(--border-strong);
    stroke-width: 1;
  }
  .axis {
    fill: var(--text-dim);
    font-family: var(--font-mono);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
  .serie {
    fill: none;
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  /* Mesures de l'hébergeur : pointillé, même couleur. */
  .serie.host {
    stroke-dasharray: 5 4;
    stroke-linecap: butt;
  }
  .serie.cpu {
    stroke: var(--viz-cpu);
  }
  .serie.ram {
    stroke: var(--viz-ram);
  }
  .serie.disk {
    stroke: var(--viz-disk);
  }
  /* Point plein, cerclé de la couleur de la carte : lisible là où les courbes se croisent. */
  .dot {
    stroke: var(--surface);
    stroke-width: 2;
  }
  .dot.cpu {
    fill: var(--viz-cpu);
  }
  .dot.ram {
    fill: var(--viz-ram);
  }
  .dot.disk {
    fill: var(--viz-disk);
  }
  .cross {
    stroke: var(--text-dim);
    stroke-width: 1;
  }
  .empty {
    padding: 22px 6px;
    font-size: 13px;
    text-align: center;
    line-height: 1.45;
  }
  .dim {
    color: var(--text-dim);
  }
</style>
