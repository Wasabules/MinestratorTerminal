<script lang="ts">
  /**
   * Paramètres du serveur — pendant tactile de la vue desktop : `server.properties` lu et modifié
   * via l'API MineStrator (`GET/PATCH /server/{id}/properties`), sans SFTP ni mot de passe.
   *
   * L'API ne modifie qu'UNE clé par appel et n'applique rien avant le prochain démarrage : chaque
   * réglage s'enregistre seul, puis un bandeau propose de redémarrer. Les interrupteurs
   * (valeurs `true`/`false`) s'enregistrent au toucher ; les autres valeurs s'éditent dans une
   * feuille, avec une saisie masquée pour les secrets (mot de passe RCON…).
   *
   * Monté en permanence dans le pager (keep-alive) : rien n'est chargé avant le premier
   * affichage de l'onglet.
   */
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import { portal } from "../../portal";
  import Icon from "../Icon.svelte";
  import Masked from "../Masked.svelte";
  import { isAnon } from "../../anon/anon.svelte";
  import { hasSensitive } from "../../anon/mask";
  import type { ServerListItem } from "../../types";

  let { server, active }: { server: ServerListItem; active: boolean } = $props();
  const serverId = $derived(server.id);

  /** Valeurs connues côté serveur (référence de l'affichage). */
  let values = $state<Record<string, string>>({});
  let loaded = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let query = $state("");
  /** Clé en cours d'enregistrement (interrupteur ou feuille). */
  let saving = $state<string | null>(null);
  let rowError = $state<Record<string, string>>({});
  /** Au moins un réglage enregistré : il ne prendra effet qu'au prochain démarrage. */
  let pendingRestart = $state(false);
  let restarting = $state(false);
  /** Confirmation demandée quand l'hébergeur a posé un avertissement sur le redémarrage. */
  let confirmRestart = $state(false);
  let notice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  /** Feuille d'édition d'une valeur texte. */
  let editing = $state<{ key: string; value: string; reveal: boolean; error: string | null } | null>(
    null,
  );

  const keys = $derived(Object.keys(values).sort());
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return q === ""
      ? keys
      : keys.filter((k) => k.toLowerCase().includes(q) || values[k].toLowerCase().includes(q));
  });

  // Premier affichage de l'onglet : chargement unique (Rafraîchir et Réessayer relancent).
  // Drapeau non réactif : un échec ne doit pas redéclencher l'effet en boucle.
  let requested = false;
  $effect(() => {
    if (active && !requested) {
      requested = true;
      void load();
    }
  });

  async function load() {
    loading = true;
    error = null;
    try {
      const p = await api.serverProperties(serverId);
      values = { ...p.properties };
      rowError = {};
      loaded = true;
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  /** Booléen tel qu'écrit dans le fichier : rendu en interrupteur. */
  function isBool(v: string | undefined): boolean {
    return v === "true" || v === "false";
  }

  /** Réglage secret (mot de passe RCON…) : jamais affiché en clair sans demande explicite. */
  function isSecret(key: string): boolean {
    return /password|secret|token/i.test(key);
  }

  /**
   * Saisie masquée dans la feuille d'édition : réglage secret, ou valeur d'origine contenant une
   * IP ou une adresse (`server-ip`…) quand le mode anonyme est actif. Lue sur la valeur enregistrée,
   * pas sur la saisie : le champ ne bascule pas pendant qu'on tape.
   */
  function isMaskedInput(key: string): boolean {
    return isSecret(key) || (isAnon() && hasSensitive(values[key]));
  }

  function isNumeric(v: string): boolean {
    return /^-?\d+$/.test(v);
  }

  function flash(msg: string) {
    notice = msg;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 3000);
  }

  /** Enregistre une valeur ; renvoie le message d'erreur éventuel. */
  async function save(key: string, value: string): Promise<string | null> {
    saving = key;
    const { [key]: _, ...others } = rowError;
    rowError = others;
    try {
      await api.setServerProperty(serverId, key, value);
      values = { ...values, [key]: value };
      pendingRestart = true;
      return null;
    } catch (e) {
      return humanizeError(e);
    } finally {
      saving = null;
    }
  }

  async function toggle(key: string) {
    if (saving) return;
    const next = values[key] === "true" ? "false" : "true";
    const err = await save(key, next);
    if (err) rowError = { ...rowError, [key]: err };
    else flash(t("props.saved"));
  }

  function openEditor(key: string) {
    editing = { key, value: values[key] ?? "", reveal: false, error: null };
  }

  async function submitEditor() {
    if (!editing || saving) return;
    const { key, value } = editing;
    if (value === values[key]) {
      editing = null;
      return;
    }
    const err = await save(key, value);
    if (err) {
      if (editing) editing.error = err;
    } else {
      editing = null;
      flash(t("props.saved"));
    }
  }

  function requestRestart() {
    if (server.restart_warning) confirmRestart = true;
    else void restart();
  }

  async function restart() {
    confirmRestart = false;
    restarting = true;
    try {
      await api.powerAction(serverId, "restart");
      pendingRestart = false;
      flash(t("props.restarted"));
    } catch (e) {
      error = humanizeError(e);
    } finally {
      restarting = false;
    }
  }

  $effect(() => () => clearTimeout(noticeTimer));
</script>

<div class="view">
  <header class="head">
    <div class="titles">
      <h2>{t("props.title")}</h2>
      <p class="dim">{t("props.subtitle")}</p>
    </div>
    <button class="ic" onclick={load} disabled={loading} aria-label={t("servers.refresh")}>
      <Icon name="refresh" size={19} />
    </button>
  </header>

  {#if notice}<p class="ok">{notice}</p>{/if}

  <!-- Bandeau de redémarrage et recherche restent en haut pendant le défilement : la liste
       compte une cinquantaine de réglages. -->
  {#if pendingRestart || (loaded && keys.length > 0 && !error)}
    <div class="top">
      {#if pendingRestart}
        <div class="banner" role="status">
          <span class="b-ico"><Icon name="info" size={18} /></span>
          <span class="b-text">{t("props.pendingRestart")}</span>
          <button class="b-btn" onclick={requestRestart} disabled={restarting}>
            <Icon name="restart" size={16} />
            {restarting ? t("props.restarting") : t("props.restartNow")}
          </button>
        </div>
      {/if}
      {#if loaded && keys.length > 0 && !error}
        <div class="search">
          <Icon name="search" size={16} />
          <input
            type="search"
            bind:value={query}
            placeholder={t("props.search")}
            aria-label={t("props.search")}
            autocapitalize="off"
            spellcheck="false"
          />
          <span class="count dim">{shown.length}/{keys.length}</span>
        </div>
      {/if}
    </div>
  {/if}

  {#if error}
    <div class="state">
      <p class="err selectable">{error}</p>
      <button class="ghost" onclick={load}>{t("props.retry")}</button>
    </div>
  {:else if !loaded}
    <p class="dim">…</p>
  {:else if keys.length === 0}
    <p class="dim">{t("props.none")}</p>
  {:else}
    {#if shown.length === 0}
      <p class="dim">{t("props.noMatch")}</p>
    {:else}
      <ul class="list">
        {#each shown as key (key)}
          {@const v = values[key]}
          <li>
            {#if isBool(v)}
              <button
                class="row"
                role="switch"
                aria-checked={v === "true"}
                onclick={() => toggle(key)}
                disabled={saving !== null}
              >
                <span class="k selectable">{key}</span>
                <span class="sw" class:on={v === "true"} class:busy={saving === key}>
                  <span class="knob"></span>
                </span>
              </button>
            {:else}
              <button class="row" onclick={() => openEditor(key)} disabled={saving !== null}>
                <span class="kv">
                  <span class="k selectable">{key}</span>
                  <span class="v" class:empty={v === ""}>
                    {#if isSecret(key) && v !== ""}
                      <Icon name="key" size={13} /> ••••••••
                    {:else if v === ""}
                      {t("props.empty")}
                    {:else}
                      <!-- Mode anonyme : IP et adresses de la valeur (server-ip…) masquées. -->
                      <Masked value={v} />
                    {/if}
                  </span>
                </span>
                <Icon name="chevronRight" size={18} />
              </button>
            {/if}
            {#if rowError[key]}<p class="err row-err selectable">{rowError[key]}</p>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

{#if editing}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => saving === null && (editing = null)}
    onkeydown={(e) => e.key === "Escape" && (editing = null)}
  ></div>
  <div class="sheet" use:portal>
    <div class="sheet-handle"></div>
    <div class="s-title selectable">{editing.key}</div>
    {#if isSecret(editing.key)}
      <p class="s-hint dim"><Icon name="key" size={13} /> {t("props.secret")}</p>
    {/if}
    <div class="field">
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="selectable"
        type={isMaskedInput(editing.key) && !editing.reveal ? "password" : "text"}
        inputmode={isNumeric(values[editing.key] ?? "") ? "numeric" : "text"}
        bind:value={editing.value}
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        autofocus
        onkeydown={(e) => e.key === "Enter" && submitEditor()}
      />
      {#if isMaskedInput(editing.key)}
        <button
          class="reveal"
          onclick={() => editing && (editing.reveal = !editing.reveal)}
          aria-label={editing.reveal ? t("props.hide") : t("props.show")}
        >
          <Icon name={editing.reveal ? "eyeOff" : "eye"} size={18} />
        </button>
      {/if}
    </div>
    {#if editing.error}<p class="err selectable">{editing.error}</p>{/if}
    <div class="s-acts">
      <button class="ghost" onclick={() => (editing = null)} disabled={saving !== null}>
        {t("props.cancel")}
      </button>
      <button
        class="primary"
        onclick={submitEditor}
        disabled={saving !== null || editing.value === values[editing.key]}
      >
        {saving ? t("props.saving") : t("props.save")}
      </button>
    </div>
  </div>
{/if}

{#if confirmRestart}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => (confirmRestart = false)}
    onkeydown={() => {}}
  ></div>
  <div class="dialog" use:portal>
    <p class="d-text selectable">{server.restart_warning}</p>
    <div class="s-acts">
      <button class="ghost" onclick={() => (confirmRestart = false)}>{t("props.cancel")}</button>
      <button class="primary" onclick={restart}>{t("props.restartNow")}</button>
    </div>
  </div>
{/if}

<style>
  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 18px;
    font-weight: 900;
  }
  .titles p {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.45;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text-muted);
  }
  .ic:disabled {
    opacity: 0.5;
  }

  /* Collé en haut de la page (le conteneur défilant du pager) ; marges négatives pour couvrir
     toute la largeur malgré le padding de `.view`, fond opaque pour masquer la liste dessous. */
  .top {
    position: sticky;
    top: 0;
    z-index: 3;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0 -16px;
    padding: 8px 16px;
    background: color-mix(in srgb, var(--bg) 94%, transparent);
    backdrop-filter: blur(8px);
  }
  .banner {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 10px;
    padding: 10px 12px;
    border: none;
    border-radius: 12px;
    background: color-mix(in srgb, var(--state-pending) 14%, var(--surface));
  }
  .b-ico {
    display: inline-flex;
    color: var(--state-pending);
  }
  .b-text {
    flex: 1 1 180px;
    font-size: 13.5px;
    font-weight: 700;
    line-height: 1.4;
  }
  .b-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    padding: 0 14px;
    border: none;
    border-radius: var(--radius);
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    font-weight: 900;
    font-size: 14px;
  }
  .b-btn:disabled {
    opacity: 0.6;
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--inset);
    color: var(--text-dim);
  }
  .search:focus-within {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 44px;
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 16px; /* 16 px : évite le zoom automatique d'Android au focus */
    outline: none;
  }
  .count {
    font-family: var(--font-mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    overflow: hidden;
  }
  li + li {
    border-top: 1px solid var(--border);
  }
  .row {
    width: 100%;
    min-height: 52px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px 8px 14px;
    border: 0;
    background: none;
    color: var(--text-dim);
    text-align: left;
  }
  .row:disabled {
    opacity: 0.7;
  }
  .kv {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .k {
    flex: 1;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: 13.5px;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .v {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .v.empty {
    color: var(--text-dim);
    font-style: italic;
  }

  /* Interrupteur : 44 × 26, l'état se lit aussi à la position du curseur (pas seulement à la couleur).
     Allumé, il prend l'accent du biome du serveur. */
  .sw {
    position: relative;
    width: 44px;
    height: 26px;
    flex: none;
    border-radius: 999px;
    background: var(--inset);
    border: 1px solid var(--border-strong);
    transition: background 0.15s;
  }
  .sw.on {
    background: var(--accent);
    border-color: transparent;
  }
  .sw.busy {
    opacity: 0.55;
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: transform 0.15s;
  }
  /* Curseur à l'encre de l'accent : visible même sur un accent très clair (neige). */
  .sw.on .knob {
    transform: translateX(18px);
    background: var(--on-accent);
  }
  .row-err {
    padding: 0 12px 10px;
  }

  .state {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }
  .ghost {
    padding: 0 16px;
    border: none;
    border-radius: var(--radius);
    background: var(--btn-secondary-bg);
    color: var(--btn-secondary-ink);
    font-weight: 800;
  }
  .dim {
    color: var(--text-dim);
  }
  .ok {
    margin: 0;
    color: var(--state-running-text);
    font-size: 14px;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
    font-size: 13px;
  }

  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    z-index: 40;
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 8px 16px calc(var(--safe-bottom) + 16px);
    background: var(--elevated);
    border-top-left-radius: var(--radius-lg);
    border-top-right-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }
  .s-title {
    font-family: var(--font-mono);
    font-size: 15px;
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .s-hint {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: -4px 0 0;
    font-size: 12.5px;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-right: 4px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--inset);
  }
  .field:focus-within {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .field input {
    flex: 1;
    min-width: 0;
    height: 48px;
    padding: 0 12px;
    border: 0;
    background: none;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 16px;
    outline: none;
  }
  .reveal {
    display: grid;
    place-items: center;
    width: 44px;
    border: 0;
    background: none;
    color: var(--text-muted);
  }
  .s-acts {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .primary {
    padding: 0 18px;
    border: none;
    border-radius: var(--radius);
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    font-weight: 900;
  }
  .primary:disabled,
  .ghost:disabled {
    opacity: 0.5;
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
  .d-text {
    margin: 0;
    font-size: 14px;
    line-height: 1.45;
    color: var(--text-muted);
    white-space: pre-line;
  }
</style>
