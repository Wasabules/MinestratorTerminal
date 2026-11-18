<script lang="ts">
  /**
   * Centre d'alertes — pendant mobile du panneau desktop : historique des alertes du superviseur
   * (crash, seuils CPU/RAM/disque, échéance des MyBox), lu dans le journal local.
   *
   * Comme sur le desktop, ouvrir l'écran marque tout comme lu ; les alertes nouvelles depuis la
   * dernière visite restent signalées le temps de la visite. « Tout effacer » vide le journal.
   * Les notifications de l'hébergeur, distinctes, ont leur propre écran (lien en tête).
   */
  import { onMount } from "svelte";
  import { t } from "../i18n";
  import { portal } from "../portal";
  import { api } from "../ipc";
  import { alerts } from "../stores/alerts.svelte";
  import Icon from "./Icon.svelte";
  import Masked from "./Masked.svelte";

  let { onBack, onNotifications }: { onBack: () => void; onNotifications: () => void } = $props();

  /** Identifiants non lus à l'ouverture : mis en évidence pendant la visite. */
  let fresh = $state<Set<number>>(new Set());
  /** `null` = tous les serveurs. */
  let serverFilter = $state<string | null>(null);
  let confirmClear = $state(false);
  /** Nombre de notifications de l'hébergeur (best-effort : elles arrivent avec le profil). */
  let hostCount = $state<number | null>(null);

  onMount(() => {
    void (async () => {
      await alerts.refresh();
      fresh = new Set(alerts.items.filter((a) => !a.read).map((a) => a.id));
      await alerts.markAllRead();
    })();
    api
      .userNotifications()
      .then((list) => (hostCount = list.length))
      .catch(() => (hostCount = null));
  });

  /** Serveurs (ou MyBox) présents dans l'historique, avec leur nombre d'alertes. */
  const sources = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const a of alerts.items) counts.set(a.server_name, (counts.get(a.server_name) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  });

  const shown = $derived(
    serverFilter === null ? alerts.items : alerts.items.filter((a) => a.server_name === serverFilter),
  );

  /** Sévérité → token d'état de la charte, toujours accompagné d'une icône et d'un libellé. */
  function color(severity: string): string {
    if (severity === "critical") return "var(--state-danger)";
    return severity === "info" ? "var(--state-running)" : "var(--state-pending)";
  }
  function severityLabel(severity: string): string {
    const k = `alerts.${severity}`;
    return t(k) !== k ? t(k) : severity;
  }
  function kindLabel(kind: string): string {
    const k = `alerts.kind.${kind}`;
    return t(k) !== k ? t(k) : kind;
  }

  /** `12/08 23:29` : même forme que le journal d'activité de l'Aperçu. */
  function when(ts: number): string {
    const d = new Date(ts * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  async function doClear() {
    confirmClear = false;
    serverFilter = null;
    await alerts.clear();
  }
</script>

<div class="screen">
  <header>
    <button class="ic" onclick={onBack} aria-label={t("common.back")}><Icon name="back" size={20} /></button>
    <span class="title">{t("alerts.title")}</span>
    {#if alerts.items.length > 0}
      <button class="ic" onclick={() => (confirmClear = true)} aria-label={t("alerts.clear")}>
        <Icon name="trash" size={19} />
      </button>
    {/if}
  </header>

  <div class="body">
    <button class="link" onclick={onNotifications}>
      <Icon name="info" size={18} />
      <span>{t("alerts.hostNotifications")}</span>
      {#if hostCount !== null}<span class="hc">{hostCount}</span>{/if}
      <Icon name="chevronRight" size={18} />
    </button>

    <p class="sub dim">{t("alerts.subtitle")}</p>

    {#if alerts.items.length === 0}
      <div class="empty">
        <p>{t("alerts.empty")}</p>
        <p class="dim">{t("alerts.emptyHint")}</p>
      </div>
    {:else}
      {#if sources.length > 1}
        <div class="chips">
          <button class="fchip" class:on={serverFilter === null} onclick={() => (serverFilter = null)}>
            {t("alerts.allServers")} <span class="n">{alerts.items.length}</span>
          </button>
          {#each sources as [name, n] (name)}
            <button class="fchip" class:on={serverFilter === name} onclick={() => (serverFilter = name)}>
              {name} <span class="n">{n}</span>
            </button>
          {/each}
        </div>
      {/if}

      <ul class="list">
        {#each shown as a (a.id)}
          <li class="item" class:fresh={fresh.has(a.id)}>
            <span class="bar" style="background:{color(a.severity)}"></span>
            <div class="c">
              <div class="top">
                <span class="srv selectable">{a.server_name}</span>
                <span class="date dim">{when(a.ts)}</span>
              </div>
              <div class="tags">
                <span class="sev" style="color:{color(a.severity)}">
                  <Icon name={a.severity === "info" ? "info" : "alert"} size={13} />
                  {severityLabel(a.severity)}
                </span>
                <span class="kind">{kindLabel(a.kind)}</span>
                {#if fresh.has(a.id)}<span class="new" aria-hidden="true"></span>{/if}
              </div>
              <!-- Mode anonyme : IP et e-mails du message masqués à l'affichage. -->
              <p class="msg selectable"><Masked value={a.message} /></p>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

{#if confirmClear}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => (confirmClear = false)}
    onkeydown={() => {}}
  ></div>
  <div class="dialog" use:portal>
    <p class="dtitle">{t("alerts.confirmClear")}</p>
    <div class="dacts">
      <button onclick={() => (confirmClear = false)}>{t("alerts.cancel")}</button>
      <button class="danger" onclick={doClear}>{t("alerts.clear")}</button>
    </div>
  </div>
{/if}

<style>
  .screen {
    display: flex;
    flex-direction: column;
    min-height: 100dvh;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: calc(var(--safe-top) + 8px) 12px 8px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text);
  }
  .title {
    flex: 1;
    font-size: 18px;
    font-weight: 900;
  }
  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px 14px calc(var(--safe-bottom) + 24px);
  }
  .link {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 48px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    color: var(--text);
    font-size: 15px;
    font-weight: 800;
    text-align: left;
  }
  .link > span:first-of-type {
    flex: 1;
  }
  .hc {
    padding: 1px 8px;
    border-radius: 999px;
    background: var(--inset);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .sub {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.45;
  }
  .empty {
    padding: 28px 8px;
    text-align: center;
  }
  .empty p {
    margin: 0 0 6px;
    font-size: 14px;
    line-height: 1.45;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .fchip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--inset);
    color: var(--text-muted);
    font-size: 13px;
    font-weight: 800;
  }
  .fchip.on {
    border-color: transparent;
    background: var(--accent);
    color: var(--on-accent);
  }
  .fchip .n {
    opacity: 0.75;
    font-family: var(--font-mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .item {
    display: flex;
    gap: 10px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .item.fresh {
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
  }
  .bar {
    width: 3px;
    border-radius: 999px;
    flex: none;
  }
  .c {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
  }
  .srv {
    font-weight: 800;
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .date {
    flex: none;
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .tags {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .sev {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-weight: 800;
  }
  .kind {
    padding: 1px 8px;
    border-radius: 999px;
    background: var(--inset);
    color: var(--text-muted);
    font-weight: 700;
  }
  .new {
    width: 8px;
    height: 8px;
    margin-left: auto;
    border-radius: 50%;
    background: var(--brand-primary);
  }
  .msg {
    margin: 0;
    font-size: 14px;
    line-height: 1.45;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }
  .dim {
    color: var(--text-dim);
  }

  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    z-index: 40;
  }
  .dialog {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 41;
    width: min(90vw, 380px);
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }
  .dtitle {
    margin: 0;
    font-size: 15px;
    font-weight: 800;
    line-height: 1.4;
  }
  .dacts {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .dacts button {
    padding: 0 16px;
    border: none;
    border-radius: var(--radius);
    background: var(--btn-secondary-bg);
    color: var(--btn-secondary-ink);
    font-weight: 800;
  }
  .dacts button.danger {
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
</style>
