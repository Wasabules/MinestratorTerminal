<script lang="ts">
  import { api, humanizeError } from "../ipc";
  import { auth } from "../stores/auth.svelte";
  import { t } from "../i18n";

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
</script>

<div class="wrap">
  <div class="hero">
    <div class="logo">⛏️</div>
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

    <button type="submit" disabled={busy || key.trim() === ""}>
      {busy ? t("onboarding.validating") : t("onboarding.submit")}
    </button>
  </form>
</div>

<style>
  .wrap {
    min-height: 100dvh;
    display: flex;
    flex-direction: column;
    justify-content: center;
    padding: calc(var(--safe-top) + 24px) 24px calc(var(--safe-bottom) + 24px);
    gap: 32px;
  }
  .hero {
    text-align: center;
  }
  .logo {
    width: 72px;
    height: 72px;
    margin: 0 auto 12px;
    display: grid;
    place-items: center;
    font-size: 34px;
    border-radius: 20px;
    background: var(--brand-gradient);
  }
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  h2 {
    margin: 0;
    font-size: 18px;
  }
  .sub {
    margin: 0;
    color: var(--text-muted);
    font-size: 14px;
  }
  input {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    padding: 14px;
    font-size: 16px;
    outline: none;
  }
  input:focus {
    border-color: var(--brand-primary);
  }
  .err {
    margin: 0;
    color: var(--state-danger);
    font-size: 14px;
  }
  button {
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: var(--radius);
    padding: 14px;
    font-weight: 600;
    font-size: 16px;
  }
  button:disabled {
    opacity: 0.5;
  }
</style>
