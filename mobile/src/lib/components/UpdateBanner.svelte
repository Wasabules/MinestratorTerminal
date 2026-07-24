<script lang="ts">
  import { api } from "../ipc";
  import { t } from "../i18n";
  import Icon from "./Icon.svelte";
  import type { UpdateInfo } from "../types";

  let info = $state<UpdateInfo | null>(null);
  let phase = $state<"idle" | "downloading" | "error">("idle");
  let dismissed = $state(false);

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
    try {
      const path = await api.downloadUpdate(info.apk_url);
      await api.installApk(path); // installeur système Android
      phase = "idle";
    } catch {
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
      {#if phase === "error"}<small class="err">{t("update.error")}</small>{/if}
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
    padding: calc(var(--safe-top) + 8px) 12px 8px;
    background: var(--brand-gradient);
    color: #fff;
  }
  .txt {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .txt strong {
    font-size: 14px;
  }
  .err {
    font-size: 11px;
    opacity: 0.9;
  }
  .go {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: rgba(255, 255, 255, 0.22);
    border: none;
    color: #fff;
    font-weight: 600;
    font-size: 13px;
    padding: 8px 12px;
    border-radius: 999px;
    flex: none;
  }
  .go:disabled {
    opacity: 0.7;
  }
  .x {
    background: transparent;
    border: none;
    color: #fff;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    flex: none;
  }
</style>
