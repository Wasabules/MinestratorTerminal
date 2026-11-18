<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";
  import { settings, type ThemePref, type LangPref } from "../stores/settings.svelte";
  import { api, openExternal, ensureNotificationPermission } from "../ipc";
  import { t } from "../i18n";
  import { isFake, setFake } from "../fake.svelte";
  import { isAnon, toggleAnon } from "../anon/anon.svelte";
  import Icon from "./Icon.svelte";
  import AccountsCard from "./AccountsCard.svelte";
  import type { UpdateInfo } from "../types";

  let { onBack }: { onBack: () => void } = $props();

  let fakeOn = $state(isFake());

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

  // Surveillance en arrière-plan (service au premier plan + notif permanente).
  let bgBusy = $state(false);
  // Fiabilité anti-kill : `true` si l'app est exemptée d'optimisation batterie, `null` si inconnu.
  let batteryOk = $state<boolean | null>(null);

  async function checkBattery() {
    try {
      batteryOk = await api.isBatteryUnrestricted();
    } catch {
      batteryOk = null;
    }
  }

  async function allowBattery() {
    // La boîte de dialogue système ouvre une autre activité ; l'état est revérifié au retour.
    await api.requestBatteryUnrestricted().catch(() => {});
  }

  async function toggleBg(on: boolean) {
    if (bgBusy) return;
    bgBusy = true;
    try {
      if (on) {
        // 1. Notifications (Android 13+) : sans quoi aucune alerte ne s'affiche.
        if (!(await ensureNotificationPermission())) {
          bgBusy = false;
          return; // permission refusée : on n'active pas
        }
        await api.setBackgroundMonitoring(true);
        settings.setBgMonitoring(true);
        // 2. Fiabilité : proposer l'exemption batterie si pas déjà accordée.
        await checkBattery();
        if (batteryOk === false) await api.requestBatteryUnrestricted().catch(() => {});
      } else {
        await api.setBackgroundMonitoring(false);
        settings.setBgMonitoring(false);
      }
    } catch {
      /* échec natif (ex. desktop) : on garde l'état précédent */
    } finally {
      bgBusy = false;
    }
  }

  // Vérifie l'état batterie à l'ouverture des réglages + au retour dans l'app (après la boîte de
  // dialogue système, la valeur a pu changer).
  $effect(() => {
    if (!settings.bgMonitoring) return;
    checkBattery();
    if (typeof document === "undefined") return;
    const onVis = () => {
      if (document.visibilityState === "visible") checkBattery();
    };
    document.addEventListener("visibilitychange", onVis);
    return () => document.removeEventListener("visibilitychange", onVis);
  });

  // Journaux anonymisés → presse-papier, à joindre à un rapport de bug.
  let logsMsg = $state<string | null>(null);
  async function copyLogs() {
    try {
      const text = await api.exportLogs();
      if (text.trim()) {
        await navigator.clipboard.writeText(text);
        logsMsg = t("settings.logsCopied");
      } else {
        logsMsg = t("settings.logsEmpty");
      }
    } catch {
      logsMsg = t("settings.logsError");
    }
    setTimeout(() => (logsMsg = null), 2500);
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
    <button class="ic" onclick={onBack} aria-label={t("common.back")}><Icon name="back" size={20} /></button>
    <span class="title">{t("settings.title")}</span>
  </header>

  <div class="body">
    <!-- Comptes MineStrator : bascule, ajout, retrait -->
    <AccountsCard />

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

    <!-- Confidentialité : mode anonyme (masque) et données de démo (substitution), côte à côte
         pour qu'on voie ce qui les distingue. -->
    <section>
      <h3>{t("settings.privacy")}</h3>
      <button class="switchrow" role="switch" aria-checked={isAnon()} onclick={toggleAnon}>
        <span class="switchtxt">
          <span class="switchlabel">
            <Icon name={isAnon() ? "eyeOff" : "eye"} size={17} />
            {t("anon.title")}
          </span>
          <span class="switchhint">{t("anon.settingDesc")}</span>
        </span>
        <span class="switch" class:on={isAnon()}><span class="knob"></span></span>
      </button>
      <button
        class="switchrow"
        role="switch"
        aria-checked={fakeOn}
        onclick={() => {
          fakeOn = !fakeOn;
          setFake(fakeOn);
        }}
      >
        <span class="switchtxt">
          <span class="switchlabel">{t("settings.fakeMode")}</span>
          <span class="switchhint">{t("settings.fakeModeHint")}</span>
        </span>
        <span class="switch" class:on={fakeOn}><span class="knob"></span></span>
      </button>
    </section>

    <section>
      <h3>{t("settings.notifications")}</h3>
      <button
        class="switchrow"
        role="switch"
        aria-checked={settings.bgMonitoring}
        disabled={bgBusy}
        onclick={() => toggleBg(!settings.bgMonitoring)}
      >
        <span class="switchtxt">
          <span class="switchlabel">{t("settings.bgMonitoring")}</span>
          <span class="switchhint">{t("settings.bgMonitoringHint")}</span>
        </span>
        <span class="switch" class:on={settings.bgMonitoring}><span class="knob"></span></span>
      </button>

      {#if settings.bgMonitoring}
        <!-- Fiabilité anti-kill : exemption batterie + accès aux réglages de l'app. -->
        <div class="reliability">
          {#if batteryOk === false}
            <button class="warnrow" onclick={allowBattery} disabled={bgBusy}>
              <Icon name="alert" size={18} />
              <span class="warntxt">
                <strong>{t("settings.batteryRestricted")}</strong>
                <small>{t("settings.batteryAllow")}</small>
              </span>
              <Icon name="chevronRight" size={16} />
            </button>
          {:else if batteryOk === true}
            <div class="okrow"><Icon name="check" size={16} /> {t("settings.batteryOk")}</div>
          {/if}
          <button class="link" onclick={() => api.openAppSettings().catch(() => {})}>
            <Icon name="settings" size={16} /> {t("settings.appSettings")}
          </button>
        </div>
      {/if}
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
      <button class="link" onclick={copyLogs}>
        <Icon name="copy" size={16} />
        {logsMsg ?? t("settings.logsCopy")}
      </button>
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
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: calc(var(--safe-top) + 6px) 10px 6px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text);
  }
  .title {
    font-size: 18px;
    font-weight: 900;
  }
  .body {
    padding: 18px 16px calc(var(--safe-bottom) + 28px);
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
    margin: 0 2px;
    font-size: 12.5px;
    font-weight: 900;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-dim);
  }
  /* Segment : la sélection prend l'accent (vert de la forêt hors d'une vue serveur). */
  .seg {
    display: flex;
    background: var(--inset);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 3px;
    gap: 2px;
  }
  .seg button {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--text-muted);
    padding: 0 8px;
    border-radius: 9px;
    font-size: 14.5px;
    font-weight: 800;
  }
  .seg button.sel {
    background: var(--accent);
    color: var(--on-accent);
  }
  .rowline {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 2px;
    font-weight: 800;
  }
  .dim {
    color: var(--text-dim);
  }
  .link {
    display: flex;
    align-items: center;
    gap: 10px;
    background: transparent;
    border: none;
    color: var(--brand-primary);
    padding: 0 2px;
    font-size: 15px;
    font-weight: 800;
    text-align: left;
  }
  .link:disabled {
    opacity: 0.6;
  }
  /* Mise à jour : bouton principal (clair en sombre, sombre en clair). */
  .update {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 48px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: var(--radius);
    padding: 0 14px;
    font-size: 15px;
    font-weight: 900;
  }
  .update:disabled {
    opacity: 0.6;
  }
  .about {
    margin: 4px 2px 0;
    color: var(--text-dim);
    font-size: 13px;
  }
  .switchrow {
    display: flex;
    align-items: center;
    gap: 14px;
    width: 100%;
    text-align: left;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 14px 16px;
    color: var(--text);
  }
  .switchrow:disabled {
    opacity: 0.6;
  }
  .switchtxt {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .switchlabel {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 15.5px;
    font-weight: 800;
  }
  .switchhint {
    font-size: 12.5px;
    color: var(--text-dim);
    line-height: 1.45;
  }
  .switch {
    flex: none;
    width: 48px;
    height: 28px;
    border-radius: 999px;
    background: var(--border-strong);
    position: relative;
    transition: background 0.18s;
  }
  .switch.on {
    background: var(--brand-primary);
  }
  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--text);
    transition: transform 0.18s, background 0.18s;
  }
  .switch.on .knob {
    transform: translateX(20px);
    background: var(--on-primary);
  }
  .reliability {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 2px;
  }
  .warnrow {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    background: color-mix(in srgb, var(--state-pending) 14%, var(--surface));
    border: 1px solid color-mix(in srgb, var(--state-pending) 40%, transparent);
    border-radius: var(--radius);
    padding: 11px 12px;
    color: var(--text);
  }
  .warnrow :global(svg:first-child) {
    color: var(--state-pending);
    flex: none;
  }
  .warntxt {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .warntxt strong {
    font-size: 14px;
  }
  .warntxt small {
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .okrow {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--state-running-text);
    font-size: 14px;
    font-weight: 800;
    padding: 4px 2px;
  }
</style>
