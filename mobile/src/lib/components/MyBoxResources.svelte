<script lang="ts">
  /**
   * Contenu de la carte « Ressources » d'une MyBox (liste des serveurs) : CPU et RAM alloués sur
   * le total, disque partagé, emplacements, serveurs avec leurs cœurs, leurs Go et leur état, et
   * renommage de la MyBox comme de ses serveurs (propriétaire uniquement).
   *
   * Unités du core : CPU en cœurs entiers, RAM en Go entiers ; seuls les serveurs actifs
   * consomment. Le disque est partagé par toute la MyBox : rien n'y est réservé par serveur.
   */
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import { settings } from "../stores/settings.svelte";
  import Icon from "./Icon.svelte";
  import type { MyBoxCapacity, MyBoxServerSlot, ResourceUsage } from "../types";

  let {
    cap,
    onChanged,
  }: {
    cap: MyBoxCapacity;
    /** Relit capacités et liste après un renommage réussi. Ne doit pas lever d'erreur. */
    onChanged: () => Promise<void>;
  } = $props();

  /** Longueur maximale d'un nom : bornée ici pour la saisie, contrôlée par le core. */
  const MAX_NAME = 50;

  /** Renommage en cours : la MyBox elle-même ou l'un de ses serveurs, avec le nom affiché. */
  let editing = $state<{ kind: "box" | "server"; id: number; current: string } | null>(null);
  let draft = $state("");
  let saving = $state(false);
  let message = $state<{ ok: boolean; text: string } | null>(null);

  /**
   * Nom enregistrable : non vide, pas trop long, et différent du nom affiché. Ce dernier point
   * évite, en mode démo, d'envoyer tel quel le nom de substitution prérempli.
   */
  const draftOk = $derived.by(() => {
    const name = draft.trim();
    return !!editing && name.length > 0 && name.length <= MAX_NAME && name !== editing.current;
  });

  /** `t()` mobile n'interpole pas : les `{x}` sont remplacés ici. */
  function tf(key: string, params: Record<string, string | number>): string {
    let s = t(key);
    for (const [k, v] of Object.entries(params)) s = s.replaceAll(`{${k}}`, String(v));
    return s;
  }

  function pct(used: number, total: number): number {
    return Math.min(100, Math.max(0, (used / total) * 100));
  }

  /** Go avec au plus une décimale, au format de la langue de l'app. */
  function gb(mb: number): string {
    return (mb / 1000).toLocaleString(settings.locale, { maximumFractionDigits: 1 });
  }

  /** CPU ou RAM : jauge `null` quand l'offre ne fixe pas de total (MyBox Pro sur mesure). */
  function alloc(r: ResourceUsage, key: "cpu" | "ram") {
    if (r.total <= 0) {
      return { detail: tf(`mybox.${key}Custom`, { allocated: r.allocated }), p: null, color: "" };
    }
    return {
      detail: tf(`mybox.${key}Detail`, {
        allocated: r.allocated,
        total: r.total,
        remaining: r.remaining,
      }),
      p: pct(r.allocated, r.total),
      // Plus rien d'allouable : impossible d'ajouter ou de reprendre un serveur.
      color: r.remaining <= 0 ? "var(--state-pending)" : "var(--chart-cpu)",
    };
  }

  const cpu = $derived(alloc(cap.cpu, "cpu"));
  const ram = $derived(alloc(cap.ram, "ram"));

  const disk = $derived.by(() => {
    const { total_gb, used_mb } = cap.disk;
    if (used_mb === null || total_gb <= 0) {
      return { detail: tf("mybox.diskTotal", { total: total_gb }), p: null, color: "" };
    }
    const p = pct(used_mb / 1000, total_gb);
    return {
      detail: tf("mybox.diskUsed", { used: gb(used_mb), total: total_gb }),
      p,
      color: p >= 90 ? "var(--state-danger)" : p >= 75 ? "var(--state-pending)" : "var(--chart-cpu)",
    };
  });

  const slots = $derived.by(() => {
    const s = cap.slots;
    if (s.max_active <= 0) {
      return {
        detail: tf("mybox.slotsUnlimited", { active: s.active, paused: s.paused }),
        p: null,
        color: "",
      };
    }
    return {
      detail: tf("mybox.slotsDetail", {
        active: s.active,
        max: s.max_active,
        paused: s.paused,
        maxPaused: s.max_paused,
      }),
      p: pct(s.active, s.max_active),
      color: s.active >= s.max_active ? "var(--state-pending)" : "var(--chart-cpu)",
    };
  });

  const anyPaused = $derived(cap.servers.some((s) => s.paused));

  /**
   * État d'un serveur. L'API marque aussi « suspendu » tout serveur en pause : la pause passe
   * donc avant la suspension.
   */
  function stateOf(s: MyBoxServerSlot): { key: string; color: string } {
    if (s.expired) return { key: "expired", color: "var(--state-danger)" };
    if (s.paused) return { key: "paused", color: "var(--state-offline)" };
    if (s.suspended) return { key: "suspended", color: "var(--state-danger)" };
    return { key: "active", color: "var(--state-running)" };
  }

  function startEdit(kind: "box" | "server", id: number, current: string) {
    editing = { kind, id, current };
    draft = current;
    message = null;
  }

  function cancelEdit() {
    editing = null;
    draft = "";
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!editing || !draftOk || saving) return;
    const target = editing;
    const name = draft.trim();
    saving = true;
    message = null;
    try {
      if (target.kind === "box") await api.renameMybox(target.id, name);
      else await api.renameServer(target.id, name);
      editing = null;
      draft = "";
      await onChanged();
      message = { ok: true, text: t("mybox.renamed") };
    } catch (err) {
      message = { ok: false, text: humanizeError(err) };
    } finally {
      saving = false;
    }
  }

  /** Ouvre le clavier sur le champ dès qu'il apparaît, nom actuel sélectionné. */
  function focusInput(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

{#snippet meter(label: string, m: { detail: string; p: number | null; color: string })}
  <div class="meter">
    <div class="mtop">
      <span class="mlabel">{label}</span>
      <span class="mdetail">{m.detail}</span>
    </div>
    {#if m.p !== null}
      <div class="bar" aria-hidden="true">
        <span style="width:{m.p}%; background:{m.color}"></span>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet renameForm()}
  <form class="rename" onsubmit={save}>
    <input
      class="selectable"
      type="text"
      maxlength={MAX_NAME}
      autocomplete="off"
      spellcheck="false"
      placeholder={t("mybox.namePlaceholder")}
      aria-label={t("mybox.namePlaceholder")}
      bind:value={draft}
      disabled={saving}
      use:focusInput
    />
    <div class="ractions">
      <button type="button" class="ghost" onclick={cancelEdit} disabled={saving}>
        {t("mybox.cancel")}
      </button>
      <button type="submit" class="primary" disabled={saving || !draftOk}>
        {saving ? t("mybox.saving") : t("mybox.save")}
      </button>
    </div>
  </form>
{/snippet}

<div class="wrap">
  {#if cap.owner}
    {#if editing?.kind === "box"}
      {@render renameForm()}
    {:else}
      <button class="link" onclick={() => startEdit("box", cap.id, cap.name)} disabled={saving}>
        <Icon name="edit" size={15} />
        {t("mybox.renameBox")}
      </button>
    {/if}
  {/if}

  <div class="meters">
    {@render meter(t("mybox.cpu"), cpu)}
    {@render meter(t("mybox.ram"), ram)}
    {@render meter(t("mybox.disk"), disk)}
    {@render meter(t("mybox.slots"), slots)}
  </div>
  {#if cap.flexcore > 0}<p class="hint">{tf("mybox.flexcore", { n: cap.flexcore })}</p>{/if}
  {#if cap.pro}<p class="hint">{t("mybox.proHint")}</p>{/if}

  <h4>{t("mybox.servers")}</h4>
  {#if cap.servers.length === 0}
    <p class="hint">{t("mybox.noServers")}</p>
  {:else}
    <ul class="srvs">
      {#each cap.servers as s (s.id)}
        {@const st = stateOf(s)}
        <li>
          {#if editing?.kind === "server" && editing.id === s.id}
            {@render renameForm()}
          {:else}
            <div class="srv">
              <span class="sname">
                <strong class="selectable">{s.name}</strong>
                <small>{s.egg_name} · {tf("mybox.serverAlloc", { cpu: s.cpu, ram: s.ram })}</small>
              </span>
              <span class="state" style="color:{st.color}">
                <span class="sdot" style="background:{st.color}"></span>{t(`mybox.state.${st.key}`)}
              </span>
              {#if cap.owner}
                <button
                  class="ic"
                  onclick={() => startEdit("server", s.id, s.name)}
                  disabled={saving}
                  aria-label={tf("mybox.renameServer", { name: s.name })}
                >
                  <Icon name="edit" size={16} />
                </button>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if anyPaused}<p class="hint">{t("mybox.pausedHint")}</p>{/if}
  {/if}

  {#if !cap.owner}<p class="hint">{t("mybox.ownerOnly")}</p>{/if}
  {#if message}<p class="msg selectable" class:ok={message.ok}>{message.text}</p>{/if}
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .meters {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .meter {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .mtop {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
  }
  .mlabel {
    flex: none;
    font-size: 13.5px;
    font-weight: 800;
    color: var(--text);
  }
  .mdetail {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-dim);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .bar {
    height: 8px;
    border-radius: 4px;
    background: var(--border-strong);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: 4px;
  }
  h4 {
    margin: 6px 0 0;
    font-weight: 900;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .srvs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--inset);
  }
  .srvs li + li {
    border-top: 1px solid var(--border);
  }
  .srvs .rename {
    padding: 10px;
  }
  .srv {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 4px 6px 10px;
    min-height: 48px;
  }
  .sname {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .sname strong {
    font-size: 14px;
    font-weight: 800;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sname small {
    font-size: 12px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .state {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    font-weight: 800;
  }
  .sdot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .ic {
    flex: none;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text-muted);
  }
  .link {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    background: transparent;
    border: none;
    color: var(--brand-primary);
    padding: 0 2px;
    font-size: 14px;
    font-weight: 800;
  }
  .rename {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .rename input {
    background: var(--inset);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    color: var(--text);
    padding: 11px 12px;
    font-size: 15px;
    outline: none;
  }
  .rename input:focus {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .ractions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .ghost {
    background: var(--btn-secondary-bg);
    border: none;
    border-radius: var(--radius);
    color: var(--btn-secondary-ink);
    padding: 0 14px;
    font-size: 14px;
    font-weight: 800;
  }
  .primary {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: var(--radius);
    padding: 0 16px;
    font-size: 14px;
    font-weight: 900;
  }
  button:disabled {
    opacity: 0.6;
  }
  .hint {
    margin: 0 2px;
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.4;
  }
  .msg {
    margin: 0 2px;
    font-size: 13px;
    color: var(--state-danger);
  }
  .msg.ok {
    color: var(--state-running-text);
  }
</style>
