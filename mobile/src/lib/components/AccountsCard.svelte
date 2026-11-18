<script lang="ts">
  /**
   * Réglages → Comptes : comptes MineStrator enregistrés sur l'appareil, bascule, ajout, retrait.
   * Les clés restent dans le Keystore Android (compte actif sous `api-key`, les autres à l'écart) ;
   * seule la liste non secrète transite ici.
   */
  import { confirm } from "@tauri-apps/plugin-dialog";
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import { applyAccountSwitch } from "../stores/accounts.svelte";
  import { avatarStyle } from "../servers/colors.svelte";
  import Icon from "./Icon.svelte";
  import type { Account } from "../types";

  let accounts = $state<Account[]>([]);
  let busy = $state<number | "add" | null>(null);
  let adding = $state(false);
  let key = $state("");
  let message = $state<{ ok: boolean; text: string } | null>(null);

  /** `t()` mobile n'interpole pas : les `{pseudo}` sont remplacés ici. */
  function tp(k: string, pseudo: string): string {
    return t(k).replace("{pseudo}", pseudo);
  }

  async function refresh() {
    try {
      accounts = await api.accountsList();
    } catch {
      /* liste indisponible : on garde la précédente */
    }
  }

  async function switchTo(a: Account) {
    busy = a.id;
    message = null;
    try {
      await applyAccountSwitch(await api.accountSwitch(a.id));
      await refresh();
    } catch (e) {
      message = { ok: false, text: humanizeError(e) };
    } finally {
      busy = null;
    }
  }

  async function remove(a: Account) {
    const ok = await confirm(
      tp(a.active ? "account.confirmRemoveActive" : "account.confirmRemove", a.pseudo),
      { kind: "warning" },
    ).catch(() => false);
    if (!ok) return;
    busy = a.id;
    message = null;
    try {
      const current = await api.accountRemove(a.id);
      if (a.active) await applyAccountSwitch(current);
      await refresh();
      message = { ok: true, text: tp("account.removed", a.pseudo) };
    } catch (e) {
      message = { ok: false, text: humanizeError(e) };
    } finally {
      busy = null;
    }
  }

  async function add(e: Event) {
    e.preventDefault();
    if (busy !== null || !key.trim()) return;
    busy = "add";
    message = null;
    try {
      const added = await api.accountAdd(key.trim());
      key = "";
      adding = false;
      // Comme sur desktop : on bascule sur le compte ajouté.
      await applyAccountSwitch(added.active ? added : await api.accountSwitch(added.id));
      await refresh();
      message = { ok: true, text: tp("account.added", added.pseudo) };
    } catch (err) {
      message = { ok: false, text: humanizeError(err) };
    } finally {
      busy = null;
    }
  }

  $effect(() => {
    void refresh();
  });
</script>

<section>
  <h3>{t("account.title")}</h3>
  <div class="list">
    {#each accounts as a (a.id)}
      <div class="row" class:active={a.active}>
        <span class="avatar" style={avatarStyle(a.pseudo)} aria-hidden="true">{a.pseudo.charAt(0).toUpperCase()}</span>
        <span class="name selectable">
          {a.pseudo}
          {#if a.active}<span class="badge">{t("account.active")}</span>{/if}
        </span>
        {#if !a.active}
          <button class="small" onclick={() => switchTo(a)} disabled={busy !== null}>
            {t("account.switch")}
          </button>
        {/if}
        <button
          class="ic danger"
          onclick={() => remove(a)}
          disabled={busy !== null}
          aria-label={t("account.remove")}
        >
          <Icon name="trash" size={18} />
        </button>
      </div>
    {/each}
  </div>

  {#if adding}
    <form class="addform" onsubmit={add}>
      <p class="hint">{t("account.addHint")}</p>
      <input
        class="selectable"
        type="password"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        placeholder={t("account.placeholder")}
        bind:value={key}
        disabled={busy !== null}
      />
      <button type="submit" class="primary" disabled={busy !== null || !key.trim()}>
        {busy === "add" ? t("account.adding") : t("account.add")}
      </button>
    </form>
  {:else}
    <button class="link" onclick={() => (adding = true)} disabled={busy !== null}>
      <Icon name="key" size={16} /> {t("account.add")}
    </button>
  {/if}

  {#if message}
    <p class="msg selectable" class:ok={message.ok}>{message.text}</p>
  {/if}
  <p class="hint">{t("account.activeHint")}</p>
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .list {
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 8px 8px 14px;
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  /* Initiale sur l'accent d'un biome tiré du pseudo (encre sombre, lisible dans les deux thèmes). */
  .avatar {
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    font-weight: 900;
    opacity: 0.7;
  }
  .row.active .avatar {
    opacity: 1;
  }
  .name {
    flex: 1;
    min-width: 0;
    font-size: 15px;
    font-weight: 800;
    display: flex;
    align-items: center;
    gap: 8px;
    overflow-wrap: anywhere;
  }
  .badge {
    font-size: 11.5px;
    font-weight: 800;
    padding: 2px 9px;
    border-radius: 999px;
    color: var(--accent-text);
    background: var(--accent-soft);
  }
  .small {
    flex: none;
    background: var(--btn-secondary-bg);
    border: none;
    border-radius: var(--radius);
    color: var(--btn-secondary-ink);
    padding: 0 14px;
    font-size: 13.5px;
    font-weight: 800;
  }
  .ic {
    flex: none;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: var(--radius);
    background: transparent;
    border: none;
    color: var(--text-muted);
  }
  .ic.danger {
    color: var(--state-danger);
  }
  button:disabled {
    opacity: 0.6;
  }
  .addform {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .addform input {
    background: var(--inset);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    color: var(--text);
    padding: 12px 14px;
    font-family: var(--font-mono);
    font-size: 15px;
    outline: none;
  }
  .addform input:focus {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .primary {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: var(--radius);
    padding: 12px;
    font-size: 15px;
    font-weight: 900;
  }
  .link {
    display: flex;
    align-items: center;
    gap: 8px;
    background: transparent;
    border: none;
    color: var(--brand-primary);
    padding: 8px 2px;
    font-size: 15px;
    font-weight: 800;
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
