<script lang="ts">
  import { api } from "../ipc";
  import { t } from "../i18n";
  import Icon from "./Icon.svelte";
  import type { UpdateInfo } from "../types";

  let info = $state<UpdateInfo | null>(null);
  let phase = $state<"idle" | "downloading" | "error">("idle");
  let dismissed = $state(false);
  let errMsg = $state(""); // diagnostic temporaire : quelle étape + message

  async function check() {
    try {
      info = await api.checkUpdate();
    } catch {
      /* silencieux : pas de réseau / rate-limit → pas de bandeau */
    }
  }

  async function update() {
    if (!info || phase === "downloading") return;
    phase = "downloading";
    errMsg = "";
    let path = "";
    try {
      path = await api.downloadUpdate(info.apk_url);
    } catch (e) {
      errMsg = "DL: " + String((e as { message?: string })?.message ?? e);
      phase = "error";
      return;
    }
    try {
      await api.installApk(path); // installeur système Android
      phase = "idle";
    } catch (e) {
      errMsg = "INSTALL: " + String((e as { message?: string })?.message ?? e);
      phase = "error";
    }
  }

  $effect(() => {
    check();
  });
</script>

{#if info && !dismissed}
  <div class="banner">
    <div class="txt">
      <strong>{t("update.available")} {info.version}</strong>
      {#if phase === "error"}<small class="err selectable">{errMsg || t("update.error")}</small>{/if}
    </div>
    <button class="go" onclick={update} disabled={phase === "downloading"}>
      {#if phase === "downloading"}
        {t("update.downloading")}
      {:else}
        <Icon name="chevronDown" size={16} /> {t("update.now")}
      {/if}
    </button>
    <button class="x" onclick={() => (dismissed = true)} aria-label={t("update.later")}>
      <Icon name="close" size={16} />
    </button>
  </div>
{/if}

<style>
  .banner {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: calc(var(--safe-top) + 8px) 8px 8px 16px;
    background: var(--elevated);
    border-bottom: 1px solid var(--border);
    box-shadow: var(--shadow);
    color: var(--text);
  }
  .txt {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .txt strong {
    font-size: 14px;
    font-weight: 900;
  }
  .err {
    font-size: 11px;
    color: var(--state-danger);
  }
  /* Bouton principal de la charte : clair en sombre, sombre en clair. */
  .go {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: var(--btn-primary-bg);
    border: none;
    color: var(--btn-primary-ink);
    font-weight: 900;
    font-size: 13.5px;
    padding: 0 14px;
    border-radius: var(--radius);
    flex: none;
  }
  .go:disabled {
    opacity: 0.7;
  }
  .x {
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text-muted);
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
  }
</style>
