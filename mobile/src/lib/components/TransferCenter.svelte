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
                <Icon name={it.direction === "up" ? "up" : "save"} size={16} />
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
  .tc-btn {
    position: relative;
  }
  /* Un transfert tourne : l'icône bat pour attirer l'œil sans bloquer l'interface. */
  .tc-btn.busy {
    color: var(--brand-primary);
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
    background: var(--brand-primary);
    color: #fff;
    font-size: 10px;
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
    background: rgb(0 0 0 / 45%);
    border: 0;
  }
  .panel {
    width: 100%;
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    border-radius: 16px 16px 0 0;
    background: var(--surface);
    padding-bottom: var(--safe-bottom);
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
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    flex: 1;
    font-size: 15px;
    font-weight: 600;
  }
  .clear {
    background: none;
    border: 0;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
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
    border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .dirico {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    flex: none;
    background: color-mix(in srgb, var(--brand-primary) 16%, transparent);
    color: var(--brand-primary);
  }
  .row.err .dirico {
    background: color-mix(in srgb, var(--state-danger) 18%, transparent);
    color: var(--state-danger);
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
  }
  .state {
    flex: none;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .speed {
    margin-left: 6px;
    color: var(--brand-primary);
  }
  .cancel {
    flex: none;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
  }
  .msg {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--state-danger);
  }

  /* Barre indéterminée : un segment qui balaie, faute de progression mesurable. */
  .bar {
    display: block;
    margin-top: 6px;
    height: 3px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    overflow: hidden;
  }
  .fill {
    display: block;
    width: 35%;
    height: 100%;
    border-radius: 999px;
    background: var(--brand-primary);
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
