<script lang="ts">
  /**
   * Centre de transferts réutilisable : une icône avec badge, et un panneau qui se déplie sur
   * la liste des envois/téléchargements. À poser dans n'importe quelle barre d'outils
   * (`<TransferCenter />`) — l'état vit dans le store global, pas dans la vue, donc un transfert
   * lancé depuis les Fichiers reste visible depuis les Sauvegardes.
   *
   * Les transferts étant découpés en blocs, la progression est **réelle** : le pourcentage
   * affiché correspond aux octets effectivement partis. La barre ne retombe sur un mode
   * indéterminé que le temps de connaître la taille totale (le `stat` distant d'un
   * téléchargement).
   */
  import { t } from "../i18n";
  import { portal } from "../portal";
  import Icon from "./Icon.svelte";
  import {
    transferItems,
    activeCount,
    clearFinished,
    fmtSpeed,
    cancel,
  } from "../stores/transfers.svelte";

  let open = $state(false);
  const items = $derived(transferItems());
  const active = $derived(activeCount());

  /** Pourcentage borné : un total qui arrive en retard ne doit pas produire de valeur absurde. */
  function pct(done: number, total: number): number {
    if (total <= 0) return 0;
    return Math.min(100, Math.max(0, Math.round((done / total) * 100)));
  }

  function fmtSize(n: number): string {
    if (n <= 0) return "—";
    const mb = n / 1024 ** 2;
    if (mb >= 1) return `${mb.toFixed(1)} Mo`;
    return `${Math.max(1, Math.round(n / 1024))} Ko`;
  }

  /** Étape d'une opération faite par le serveur (compression, extraction). */
  function phaseLabel(phase: string | null | undefined, files: number | undefined): string {
    switch (phase) {
      case "queued":
        return t("transfers.phase.queued");
      case "compressing":
        return files
          ? t("transfers.phase.compressingCount").replace("{n}", String(files))
          : t("transfers.phase.compressing");
      case "extracting":
        return t("transfers.phase.extracting");
      default:
        return "";
    }
  }
</script>

<button
  class="ic tc-btn"
  class:busy={active > 0}
  onclick={() => (open = true)}
  aria-label={t("transfers.title")}
>
  <Icon name="save" size={18} />
  {#if active > 0}
    <span class="badge">{active}</span>
  {:else if items.length > 0}
    <span class="dot"></span>
  {/if}
</button>

{#if open}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => (open = false)}
    onkeydown={() => {}}
  >
    <div class="panel" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="sheet-handle"></div>
      <header>
        <span class="title">{t("transfers.title")}</span>
        {#if items.some((i) => i.status !== "active")}
          <button class="clear" onclick={clearFinished}>{t("transfers.clear")}</button>
        {/if}
        <button class="ic" onclick={() => (open = false)} aria-label={t("files.close")}>
          <Icon name="close" size={18} />
        </button>
      </header>

      {#if items.length === 0}
        <p class="empty dim">{t("transfers.empty")}</p>
      {:else}
        <ul>
          {#each items as it (it.id)}
            <li class="row" class:err={it.status === "error"}>
              <span class="dirico" class:up={it.direction === "up"}>
                <Icon name={it.direction === "up" ? "up" : it.direction === "server" ? "archive" : "save"} size={16} />
              </span>
              <span class="body">
                <span class="top">
                  <span class="name">{it.name}</span>
                  <span class="state dim">
                    {#if it.status === "active"}
                      {#if it.total > 0}
                        {pct(it.done, it.total)}% · {fmtSize(it.done)} / {fmtSize(it.total)}
                      {:else if it.done > 0}
                        <!-- Taille inconnue (certains fournisseurs Android ne la déclarent pas) :
                             on montre au moins ce qui est parti, sinon rien ne distingue un
                             transfert qui avance d'un transfert bloqué. -->
                        {fmtSize(it.done)}
                      {:else}
                        {t("transfers.running")}
                      {/if}
                      {#if fmtSpeed(it.speed)}
                        <span class="speed">{fmtSpeed(it.speed)}</span>
                      {/if}
                    {:else if it.status === "done"}
                      {fmtSize(it.size)}
                    {:else}
                      {t("transfers.failed")}
                    {/if}
                  </span>
                </span>
                {#if it.status === "active"}
                  <!-- Déterminée dès que le total est connu ; indéterminée le temps du `stat`. -->
                  <span class="bar">
                    {#if it.total > 0}
                      <span class="fill known" style="width:{pct(it.done, it.total)}%"></span>
                    {:else}
                      <span class="fill"></span>
                    {/if}
                  </span>
                  {#if it.direction === "server" && phaseLabel(it.phase, it.files)}
                    <small class="phase dim">{phaseLabel(it.phase, it.files)}</small>
                  {/if}
                {:else if it.error}
                  <small class="msg selectable">{it.error}</small>
                {/if}
              </span>
              {#if it.status === "active"}
                <button
                  class="cancel"
                  onclick={() => cancel(it.id)}
                  aria-label={t("transfers.cancel")}
                  title={t("transfers.cancel")}
                >
                  <Icon name="close" size={16} />
                </button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
{/if}

<style>
  /* Bouton posé dans les barres d'outils des vues (Fichiers, Sauvegardes) : leurs styles,
     scopés, ne l'atteignent pas, d'où une forme propre. */
  .tc-btn {
    position: relative;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text-muted);
  }
  /* Un transfert tourne : l'icône bat pour attirer l'œil sans bloquer l'interface. */
  .tc-btn.busy {
    color: var(--accent-text);
    animation: tc-pulse 1.4s ease-in-out infinite;
  }
  @keyframes tc-pulse {
    0%,
    100% {
      opacity: 1;
      transform: scale(1);
    }
    50% {
      opacity: 0.45;
      transform: scale(0.9);
    }
  }
  /* Le badge chiffré bat aussi, mais en décalé : sinon l'ensemble « disparaît » d'un bloc. */
  .tc-btn.busy .badge {
    animation: tc-badge 1.4s ease-in-out infinite;
  }
  @keyframes tc-badge {
    0%,
    100% {
      transform: scale(1);
    }
    50% {
      transform: scale(1.15);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .tc-btn.busy,
    .tc-btn.busy .badge {
      animation: none;
    }
  }
  .badge {
    position: absolute;
    top: 2px;
    right: 2px;
    min-width: 15px;
    height: 15px;
    padding: 0 3px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--on-accent);
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 800;
    line-height: 15px;
    font-variant-numeric: tabular-nums;
  }
  /* Aucun transfert en cours mais un historique : simple point, sans chiffre. */
  .dot {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-dim);
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: flex;
    align-items: flex-end;
    background: var(--scrim);
    border: 0;
  }
  .panel {
    width: 100%;
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    background: var(--elevated);
    box-shadow: var(--shadow);
    padding: 8px 0 var(--safe-bottom);
    animation: slide-up 0.18s ease-out;
  }
  @keyframes slide-up {
    from {
      transform: translateY(12%);
      opacity: 0.6;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .panel {
      animation: none;
    }
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 8px 8px 16px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    flex: 1;
    font-size: 16px;
    font-weight: 900;
  }
  .clear {
    background: none;
    border: 0;
    color: var(--brand-primary);
    font: inherit;
    font-size: 13.5px;
    font-weight: 800;
    padding: 0 8px;
  }
  header .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text-muted);
  }
  .empty {
    padding: 28px 14px;
    text-align: center;
    font-size: 14px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
  }
  .row {
    display: flex;
    gap: 10px;
    padding: 11px 14px;
    align-items: center;
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .dirico {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    flex: none;
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .row.err .dirico {
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
  .body {
    flex: 1;
    min-width: 0;
  }
  .top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 14px;
    font-weight: 800;
  }
  .state {
    flex: none;
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .speed {
    margin-left: 6px;
    color: var(--accent-text);
  }
  .cancel {
    flex: none;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: var(--radius);
    border: none;
    background: var(--btn-secondary-bg);
    color: var(--btn-secondary-ink);
  }
  .msg {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--state-danger);
  }
  .phase {
    display: block;
    margin-top: 4px;
    font-size: 12px;
  }

  /* Barre indéterminée : un segment qui balaie, faute de progression mesurable. */
  .bar {
    display: block;
    margin-top: 6px;
    height: 4px;
    border-radius: 999px;
    background: var(--border-strong);
    overflow: hidden;
  }
  .fill {
    display: block;
    width: 35%;
    height: 100%;
    border-radius: 999px;
    background: var(--accent);
    animation: sweep 1.1s ease-in-out infinite;
  }
  /* Progression connue : largeur pilotée, pas d'animation de balayage. */
  .fill.known {
    animation: none;
    transition: width 0.2s linear;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(320%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .fill {
      animation: none;
      width: 100%;
      opacity: 0.5;
    }
  }
</style>
