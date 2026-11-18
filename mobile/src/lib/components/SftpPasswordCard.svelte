<script lang="ts">
  /**
   * Saisie du mot de passe SFTP d'un serveur, en secours de celui fourni par l'API. Le cœur le
   * vérifie en ouvrant une session, puis le range dans le Keystore Android ; il ne revient jamais
   * au JS.
   */
  import { onMount } from "svelte";
  import { api, humanizeError } from "../ipc";
  import type { AppError } from "../types";
  import { t } from "../i18n";

  let {
    serverId,
    reason,
    onsaved,
    onclose,
  }: {
    serverId: number;
    /** `required` : aucun mot de passe ; `rejected` : refusé ; `change` : ouvert par l'utilisateur. */
    reason: "required" | "rejected" | "change";
    onsaved: () => void;
    onclose: () => void;
  } = $props();

  let value = $state("");
  let show = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let note = $state<string | null>(null);
  let stored = $state(false);

  onMount(async () => {
    try {
      stored = await api.sftpHasPassword(serverId);
    } catch {
      stored = false;
    }
  });

  const intro = $derived(
    reason === "rejected"
      ? t("files.password.rejected")
      : reason === "change" && stored
        ? t("files.password.change")
        : t("files.password.required"),
  );

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!value || busy) return;
    busy = true;
    err = null;
    note = null;
    try {
      await api.sftpSetPassword(serverId, value);
      value = "";
      onsaved();
    } catch (error) {
      const kind = (error as Partial<AppError> | undefined)?.kind;
      err = kind === "sftp_auth" ? t("files.password.rejected") : humanizeError(error);
    } finally {
      busy = false;
    }
  }

  async function forget() {
    err = null;
    try {
      await api.sftpForgetPassword(serverId);
      stored = false;
      note = t("files.password.forgotten");
    } catch (error) {
      err = humanizeError(error);
    }
  }
</script>

<form class="card" onsubmit={save}>
  <h3>{t("files.password.title")}</h3>
  <p class="intro selectable" class:warn={reason === "rejected"}>{intro}</p>
  <p class="hint selectable">{t("files.password.where")}</p>
  <div class="field">
    <input
      class="selectable"
      type={show ? "text" : "password"}
      inputmode="text"
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      placeholder={t("files.password.placeholder")}
      bind:value
      disabled={busy}
    />
    <button type="button" class="reveal" onclick={() => (show = !show)}>
      {show ? t("files.password.hide") : t("files.password.show")}
    </button>
  </div>
  <p class="hint">{t("files.password.stored")}</p>
  {#if err}<p class="msg err selectable">{err}</p>{/if}
  {#if note}<p class="msg">{note}</p>{/if}
  <button type="submit" class="primary" disabled={!value || busy}>
    {busy ? t("files.password.checking") : t("files.password.save")}
  </button>
  <div class="row">
    {#if stored}
      <button type="button" class="link" onclick={forget} disabled={busy}>{t("files.password.forget")}</button>
    {/if}
    <span class="grow"></span>
    <button type="button" class="link" onclick={onclose} disabled={busy}>{t("files.password.cancel")}</button>
  </div>
</form>

<style>
  .card {
    margin: 8px 12px;
    padding: 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 17px;
    font-weight: 900;
  }
  .intro {
    margin: 0;
    font-size: 14px;
  }
  .intro.warn {
    color: var(--state-danger);
  }
  .hint {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .field {
    display: flex;
    gap: 8px;
  }
  input {
    flex: 1;
    min-width: 0;
    background: var(--inset);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    color: var(--text);
    padding: 12px;
    font-family: var(--font-mono);
    font-size: 16px;
    outline: none;
  }
  input:focus {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .reveal {
    background: var(--btn-secondary-bg);
    border: none;
    border-radius: var(--radius);
    color: var(--btn-secondary-ink);
    font-weight: 800;
    padding: 0 14px;
  }
  .msg {
    margin: 0;
    font-size: 14px;
    color: var(--state-running-text);
  }
  .msg.err {
    color: var(--state-danger);
  }
  button.primary {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: var(--radius);
    padding: 12px;
    font-weight: 900;
    font-size: 16px;
  }
  button.primary:disabled {
    opacity: 0.5;
  }
  .row {
    display: flex;
    align-items: center;
  }
  .grow {
    flex: 1;
  }
  button.link {
    background: transparent;
    border: none;
    color: var(--brand-primary);
    font-size: 15px;
    font-weight: 800;
    padding: 8px;
  }
</style>
