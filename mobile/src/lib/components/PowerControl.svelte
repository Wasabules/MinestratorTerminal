<script lang="ts">
  /**
   * Alimentation d'un serveur, posée sur le paysage de l'en-tête (toujours sombre).
   *
   * - `variant="row"` (bannière de l'Aperçu) : boutons visibles — serveur en marche « Redémarrer »
   *   (principal, clair) puis « Arrêter » ; arrêté, « Démarrer » — et « ▾ » qui ouvre le reste.
   * - `variant="icon"` (barre compacte des autres vues) : un seul bouton qui ouvre toutes les
   *   actions.
   *
   * Les actions secondaires s'ouvrent dans une feuille du bas (atteignable au pouce), « Forcer
   * l'arrêt » en rouge. Un avertissement configuré par l'hébergeur (`restart_warning`,
   * `stop_warning`) passe par une confirmation qui l'affiche.
   */
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import { portal } from "../portal";
  import type { PowerAction } from "../types";
  import Icon from "./Icon.svelte";

  let {
    serverId,
    runState,
    restartWarning = null,
    stopWarning = null,
    variant = "row",
    onDone,
  }: {
    serverId: number;
    /** État d'exécution (`running`, `offline`, `starting`, `stopping`…) ; `null` : inconnu. */
    runState: string | null;
    restartWarning?: string | null;
    stopWarning?: string | null;
    variant?: "row" | "icon";
    /** Appelé après une action acceptée par l'API (relecture de l'état). */
    onDone?: () => void;
  } = $props();

  /**
   * Processus présent (en marche, ou en train de démarrer ou de s'arrêter) : redémarrer, arrêter
   * et forcer l'arrêt ont un sens — un serveur bloqué en « Arrêt… » se débloque d'un arrêt forcé.
   */
  const alive = $derived(runState === "running" || runState === "starting" || runState === "stopping");
  const known = $derived(runState !== null);

  /** Boutons visibles (le premier est le principal, clair). */
  const buttons = $derived<PowerAction[]>(alive ? ["restart", "stop"] : ["start"]);
  /**
   * Reste des actions, dans la feuille. État inconnu (aucun échantillon reçu) : toutes, plutôt
   * que de masquer à tort ce dont l'utilisateur a besoin.
   */
  const secondary = $derived<PowerAction[]>(
    alive ? ["kill"] : known ? ["restart"] : ["restart", "stop", "kill"],
  );
  /** Feuille : en variante icône, toutes les actions ; sinon, seulement le reste. */
  const sheetActions = $derived(variant === "icon" ? [...buttons, ...secondary] : secondary);

  const ICONS: Record<PowerAction, string> = {
    start: "play",
    restart: "restart",
    stop: "stop",
    kill: "kill",
  };

  let busy = $state<PowerAction | null>(null);
  let sheetOpen = $state(false);
  /** Action retenue en attente de confirmation (avertissement de l'hébergeur à lire). */
  let pending = $state<PowerAction | null>(null);
  let error = $state<string | null>(null);
  let errTimer: ReturnType<typeof setTimeout> | undefined;

  function warningFor(action: PowerAction): string | null {
    if (action === "restart") return restartWarning || null;
    if (action === "stop" || action === "kill") return stopWarning || null;
    return null;
  }

  function request(action: PowerAction) {
    sheetOpen = false;
    if (warningFor(action)) {
      pending = action;
      return;
    }
    void act(action);
  }

  async function act(action: PowerAction) {
    pending = null;
    if (busy) return;
    busy = action;
    error = null;
    try {
      await api.powerAction(serverId, action);
      onDone?.();
    } catch (e) {
      showError(humanizeError(e));
    } finally {
      busy = null;
    }
  }

  function showError(msg: string) {
    error = msg;
    clearTimeout(errTimer);
    errTimer = setTimeout(() => (error = null), 6000);
  }

  $effect(() => () => clearTimeout(errTimer));
</script>

{#if variant === "row"}
  <div class="row">
    {#each buttons as action, i (action)}
      <button class="pbtn" class:main={i === 0} disabled={busy !== null} onclick={() => request(action)}>
        {#if busy === action}<span class="spinner"></span>{:else}<Icon name={ICONS[action]} size={17} />{/if}
        <span>{t(`power.${action}`)}</span>
      </button>
    {/each}
    <button
      class="pbtn caret"
      disabled={busy !== null}
      aria-label={t("power.more")}
      aria-haspopup="menu"
      aria-expanded={sheetOpen}
      onclick={() => (sheetOpen = true)}
    >
      <Icon name="chevronDown" size={20} />
    </button>
  </div>
{:else}
  <button
    class="ibtn"
    disabled={busy !== null}
    aria-label={t("power.title")}
    aria-haspopup="menu"
    aria-expanded={sheetOpen}
    onclick={() => (sheetOpen = true)}
  >
    {#if busy}<span class="spinner"></span>{:else}<Icon name="kill" size={20} />{/if}
  </button>
{/if}

{#if sheetOpen}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    aria-label={t("common.close")}
    onclick={() => (sheetOpen = false)}
    onkeydown={(e) => e.key === "Escape" && (sheetOpen = false)}
  ></div>
  <div class="sheet" use:portal role="menu" aria-label={t("power.title")}>
    <div class="sheet-handle"></div>
    <div class="sheet-title">{t("power.title")}</div>
    {#each sheetActions as action (action)}
      <button role="menuitem" class="item" class:danger={action === "kill"} onclick={() => request(action)}>
        <span class="ig"><Icon name={ICONS[action]} size={19} /></span>
        <span>{t(`power.${action}`)}</span>
      </button>
    {/each}
  </div>
{/if}

{#if pending}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    aria-label={t("common.close")}
    onclick={() => (pending = null)}
    onkeydown={(e) => e.key === "Escape" && (pending = null)}
  ></div>
  <div class="sheet" use:portal role="dialog" aria-modal="true" aria-label={t(`power.${pending}`)}>
    <div class="sheet-handle"></div>
    <p class="warn selectable">{warningFor(pending)}</p>
    <div class="confirm">
      <button class="btn btn--ghost" onclick={() => (pending = null)}>{t("backups.cancel")}</button>
      <button class="btn" class:btn--danger={pending === "kill"} onclick={() => act(pending!)}>
        {t(`power.${pending}`)}
      </button>
    </div>
  </div>
{/if}

{#if error}
  <button class="err selectable" use:portal onclick={() => (error = null)}>{error}</button>
{/if}

<style>
  /* Sur le paysage : couleurs du thème sombre dans les deux thèmes (principal clair, secondaire
     noir), comme la bannière desktop. */
  .row {
    display: flex;
    gap: 8px;
    width: 100%;
  }
  .pbtn {
    flex: 1;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 46px;
    padding: 0 12px;
    border: none;
    border-radius: var(--radius);
    font-weight: 800;
    font-size: 15px;
    white-space: nowrap;
    color: #f1f5f3;
    background: #0d1113;
  }
  .pbtn span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pbtn.main {
    font-weight: 900;
    color: #0d1113;
    background: #f1f5f3;
  }
  .pbtn:active:not(:disabled) {
    background: #1b2126;
  }
  .pbtn.main:active:not(:disabled) {
    background: #dfe6e2;
  }
  .pbtn.caret {
    flex: none;
    width: 46px;
    padding: 0;
  }
  .pbtn:disabled,
  .ibtn:disabled {
    opacity: 0.55;
  }
  .ibtn {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    padding: 0;
    border: none;
    border-radius: var(--radius);
    color: #e4ebe8;
    background: rgba(13, 17, 19, 0.72);
  }
  .pbtn .spinner,
  .ibtn .spinner {
    border-color: color-mix(in srgb, currentColor 25%, transparent);
    border-top-color: currentColor;
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: var(--scrim);
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px calc(var(--safe-bottom) + 14px);
    background: var(--elevated);
    border-top-left-radius: var(--radius-lg);
    border-top-right-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    color: var(--text);
  }
  .sheet-title {
    padding: 4px 12px 8px;
    color: var(--text-dim);
    font-size: 13px;
    font-weight: 800;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 14px;
    min-height: 54px;
    padding: 0 12px;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text);
    font-size: 16px;
    font-weight: 800;
    text-align: left;
  }
  .item:active {
    background: var(--hover);
  }
  .ig {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: var(--radius);
    background: var(--inset);
    color: var(--text-muted);
  }
  .item.danger {
    color: var(--state-danger);
  }
  .item.danger .ig {
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
  .warn {
    margin: 4px 6px 14px;
    font-size: 14.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  .confirm {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    padding: 0 4px;
  }
  /* Erreur : pilule flottante sous l'en-tête, touchée pour disparaître. */
  .err {
    position: fixed;
    z-index: 45;
    top: calc(var(--safe-top) + 64px);
    left: 16px;
    right: 16px;
    padding: 10px 14px;
    border: none;
    border-radius: var(--radius);
    background: var(--danger-bg);
    color: var(--danger-ink);
    box-shadow: var(--shadow);
    font-size: 14px;
    font-weight: 800;
    text-align: left;
  }
</style>
