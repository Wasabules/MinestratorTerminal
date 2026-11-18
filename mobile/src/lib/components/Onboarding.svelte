<script lang="ts">
  import { api, humanizeError, openExternal } from "../ipc";
  import { auth } from "../stores/auth.svelte";
  import { t } from "../i18n";
  import Icon from "./Icon.svelte";
  import Logo from "./Logo.svelte";
  import BiomeLandscape from "./BiomeLandscape.svelte";
  import { BIOMES } from "../servers/colors.svelte";

  let key = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(e: Event) {
    e.preventDefault();
    if (busy || key.trim() === "") return;
    busy = true;
    error = null;
    try {
      const user = await api.validateAndStoreKey(key.trim());
      auth.setUser(user);
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = false;
    }
  }

  function openSite() {
    openExternal("https://minestrator.com").catch(() => {});
  }
</script>

<div class="wrap">
  <!-- Même accueil que l'écran de connexion desktop : paysage de la forêt, logo « curseur ». -->
  <div class="cover" style="background: {BIOMES[0].sky}">
    <BiomeLandscape biome={BIOMES[0]} seed={7} height={132} pixel={6} />
  </div>
  <div class="hero">
    <div class="logo"><Logo size={40} /></div>
    <h1>{t("app.title")}</h1>
  </div>

  <form onsubmit={submit}>
    <h2>{t("onboarding.title")}</h2>
    <p class="sub selectable">{t("onboarding.subtitle")}</p>

    <input
      class="selectable"
      type="password"
      inputmode="text"
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      placeholder={t("onboarding.placeholder")}
      bind:value={key}
      disabled={busy}
    />

    {#if error}
      <p class="err selectable">{error}</p>
    {/if}

    <button type="submit" class="primary" disabled={busy || key.trim() === ""}>
      {busy ? t("onboarding.validating") : t("onboarding.submit")}
    </button>

    <button type="button" class="link" onclick={openSite}>
      <Icon name="external" size={16} />
      {t("onboarding.getKey")}
    </button>
  </form>
</div>

<style>
  .wrap {
    min-height: 100dvh;
    display: flex;
    flex-direction: column;
    padding: 0 24px calc(var(--safe-bottom) + 24px);
    gap: 28px;
  }
  /* Paysage en bandeau du haut (décoratif), bord à bord ; le ciel passe sous la barre d'état. */
  .cover {
    margin: 0 -24px;
    padding-top: var(--safe-top);
  }
  /* Le logo chevauche le bas du paysage. */
  .hero {
    position: relative;
    text-align: center;
    margin-top: -66px;
  }
  /* Tuile du logo : toujours sombre (DESIGN.md), posée à cheval sur le paysage. */
  .logo {
    width: 76px;
    height: 76px;
    margin: 0 auto 14px;
    display: grid;
    place-items: center;
    border-radius: 20px;
    background: #0d1113;
    border: 3px solid var(--bg);
  }
  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 900;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 900;
  }
  .sub {
    margin: 0;
    color: var(--text-muted);
    font-size: 14px;
  }
  input {
    background: var(--inset);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    color: var(--text);
    padding: 14px;
    font-family: var(--font-mono);
    font-size: 16px;
    outline: none;
  }
  input:focus {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .err {
    margin: 0;
    color: var(--state-danger);
    font-size: 14px;
  }
  button.primary {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: var(--radius);
    padding: 14px;
    font-weight: 900;
    font-size: 16px;
  }
  button.primary:disabled {
    opacity: 0.5;
  }
  button.link {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    background: transparent;
    border: none;
    color: var(--brand-primary);
    font-size: 14px;
    font-weight: 800;
    padding: 8px;
  }
</style>
