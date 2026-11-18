<script lang="ts">
  /**
   * Changement d'identité du serveur SFTP : le cœur a refusé la connexion parce que l'empreinte de
   * la clé d'hôte n'est plus celle mémorisée (le mot de passe n'a pas été envoyé). L'utilisateur
   * compare et décide ; seule l'empreinte réellement présentée peut être acceptée.
   */
  import { onMount } from "svelte";
  import { api, humanizeError } from "../ipc";
  import type { SftpHostKey } from "../types";
  import { t } from "../i18n";

  let {
    serverId,
    ontrusted,
    onclose,
  }: { serverId: number; ontrusted: () => void; onclose: () => void } = $props();

  let info = $state<SftpHostKey | null>(null);
  let busy = $state(false);
  let err = $state<string | null>(null);

  onMount(async () => {
    try {
      info = await api.sftpHostKey(serverId);
    } catch (e) {
      err = humanizeError(e);
    }
  });

  async function trust() {
    const fingerprint = info?.presented_fingerprint;
    if (!fingerprint || busy) return;
    busy = true;
    err = null;
    try {
      await api.sftpTrustHostKey(serverId, fingerprint);
      ontrusted();
    } catch (e) {
      err = humanizeError(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="card" role="alertdialog" aria-labelledby="hk-title">
  <h3 id="hk-title">{t("files.hostKey.title")}</h3>
  <p class="intro selectable">{t("files.hostKey.intro")}</p>
  <p class="hint selectable">{t("files.hostKey.causes")}</p>
  {#if info}
    <div class="field">
      <span class="label">{t("files.hostKey.host")}</span>
      <span class="mono selectable">{info.host}</span>
    </div>
    <div class="field">
      <span class="label">{t("files.hostKey.known")}</span>
      <span class="mono selectable">{info.known_fingerprint ?? "—"}</span>
    </div>
    <div class="field">
      <span class="label">{t("files.hostKey.presented")}</span>
      <span class="mono selectable new">{info.presented_fingerprint ?? t("files.hostKey.none")}</span>
    </div>
  {/if}
  {#if err}<p class="msg err selectable">{err}</p>{/if}
  <button type="button" class="danger" onclick={trust} disabled={busy || !info?.presented_fingerprint}>
    {busy ? t("files.hostKey.trusting") : t("files.hostKey.trust")}
  </button>
  <button type="button" class="link" onclick={onclose} disabled={busy}>{t("files.hostKey.cancel")}</button>
</div>

<style>
  .card {
    margin: 8px 12px;
    padding: 16px;
    background: var(--surface);
    border: 1px solid var(--state-danger);
    border-radius: var(--radius);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 17px;
    color: var(--state-danger);
  }
  .intro {
    margin: 0;
    font-size: 14px;
  }
  .hint {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .label {
    font-size: 12px;
    color: var(--text-muted);
  }
  .mono {
    font-family: ui-monospace, monospace;
    font-size: 13px;
    overflow-wrap: anywhere;
  }
  .new {
    color: var(--state-danger);
  }
  .msg {
    margin: 0;
    font-size: 14px;
  }
  .msg.err {
    color: var(--state-danger);
  }
  button.danger {
    background: var(--state-danger);
    color: #fff;
    border: none;
    border-radius: var(--radius);
    padding: 12px;
    font-weight: 600;
    font-size: 15px;
  }
  button.danger:disabled {
    opacity: 0.5;
  }
  button.link {
    background: transparent;
    border: none;
    color: var(--brand-primary);
    font-size: 15px;
    padding: 8px;
  }
</style>
