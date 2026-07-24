<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";
  import { settings, type ThemePref, type LangPref } from "../stores/settings.svelte";
  import { api, openExternal } from "../ipc";
  import { t } from "../i18n";
  import Icon from "./Icon.svelte";
  import type { UpdateInfo } from "../types";

  let { onBack }: { onBack: () => void } = $props();

  let version = $state("0.4.0");
  $effect(() => {
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
  });

  // Recherche manuelle de MAJ.
  let upd = $state<"idle" | "checking" | "uptodate" | "downloading">("idle");
  let updInfo = $state<UpdateInfo | null>(null);

  async function checkUpdate() {
    upd = "checking";
    updInfo = null;
    try {
      updInfo = await api.checkUpdate();
      upd = updInfo ? "idle" : "uptodate";
    } catch {
      upd = "uptodate";
    }
  }
  async function doUpdate() {
    if (!updInfo) return;
    upd = "downloading";
    try {
      const path = await api.downloadUpdate(updInfo.apk_url);
      await api.installApk(path);
      upd = "idle";
    } catch {
      upd = "idle";
    }
  }

  const themeOpts: { val: ThemePref; key: string }[] = [
    { val: "system", key: "settings.system" },
    { val: "dark", key: "settings.dark" },
    { val: "light", key: "settings.light" },
  ];
  const langOpts: { val: LangPref; key: string }[] = [
    { val: "system", key: "settings.system" },
    { val: "fr", key: "settings.french" },
    { val: "en", key: "settings.english" },
  ];
</script>

<div class="screen">
  <header>
    <button class="ic" onclick={onBack} aria-label="Retour"><Icon name="back" size={20} /></button>
    <span class="title">{t("settings.title")}</span>
  </header>

  <div class="body">
    <!-- Thème -->
    <section>
      <h3>{t("settings.theme")}</h3>
      <div class="seg">
        {#each themeOpts as o (o.val)}
          <button class:sel={settings.theme === o.val} onclick={() => settings.setTheme(o.val)}>
            {t(o.key)}
          </button>
        {/each}
      </div>
    </section>

    <!-- Langue -->
    <section>
      <h3>{t("settings.language")}</h3>
      <div class="seg">
        {#each langOpts as o (o.val)}
          <button class:sel={settings.lang === o.val} onclick={() => settings.setLang(o.val)}>
            {t(o.key)}
          </button>
        {/each}
      </div>
    </section>

    <!-- Version + liens -->
    <section>
      <div class="rowline">
        <span>{t("settings.version")}</span>
        <span class="dim selectable">v{version}</span>
      </div>
      <button
        class="link"
        onclick={checkUpdate}
        disabled={upd === "checking" || upd === "downloading"}
      >
        <Icon name="refresh" size={16} />
        {upd === "checking"
          ? t("settings.checking")
          : upd === "uptodate"
            ? t("settings.upToDate")
            : t("settings.checkUpdate")}
      </button>
      {#if updInfo}
        <button class="update" onclick={doUpdate} disabled={upd === "downloading"}>
          <Icon name="chevronDown" size={16} />
          {upd === "downloading" ? t("update.downloading") : `${t("update.now")} → ${updInfo.version}`}
        </button>
      {/if}
      <button class="link" onclick={() => openExternal("https://minestrator.com").catch(() => {})}>
        <Icon name="external" size={16} /> {t("settings.website")}
      </button>
      <p class="about">{t("settings.about")}</p>
    </section>
  </div>
</div>

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
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .ic {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    background: transparent;
    border: none;
    color: var(--text);
  }
  .title {
    font-size: 17px;
    font-weight: 600;
  }
  .body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 26px;
  }
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
  .seg {
    display: flex;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 4px;
    gap: 4px;
  }
  .seg button {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--text-muted);
    padding: 11px;
    border-radius: var(--radius-sm);
    font-size: 14px;
    font-weight: 600;
  }
  .seg button.sel {
    background: var(--brand-primary);
    color: #fff;
  }
  .rowline {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 2px;
  }
  .dim {
    color: var(--text-dim);
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
  }
  .link:disabled {
    opacity: 0.6;
  }
  .update {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: var(--radius);
    padding: 12px;
    font-size: 15px;
    font-weight: 600;
  }
  .update:disabled {
    opacity: 0.6;
  }
  .about {
    margin: 4px 2px 0;
    color: var(--text-dim);
    font-size: 13px;
  }
</style>
