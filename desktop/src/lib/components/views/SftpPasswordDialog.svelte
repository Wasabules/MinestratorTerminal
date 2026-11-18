<script lang="ts">
  /**
   * Saisie du mot de passe SFTP d'un serveur, en secours de celui fourni par l'API :
   * l'utilisateur le copie depuis l'onglet « Accès » du panel. Le cœur le vérifie en ouvrant une
   * session avant de l'enregistrer dans le stockage sécurisé de l'OS ; il ne revient jamais au JS.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError, isAppError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';

  let {
    serverId,
    reason,
    onsaved,
    onclose,
  }: {
    serverId: number;
    /** `required` : aucun mot de passe ; `rejected` : refusé ; `change` : ouvert par l'utilisateur. */
    reason: 'required' | 'rejected' | 'change';
    onsaved: () => void;
    onclose: () => void;
  } = $props();

  let value = $state('');
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
    reason === 'rejected'
      ? t('sftp.password.rejected')
      : reason === 'required'
        ? t('sftp.password.required')
        : stored
          ? t('sftp.password.change')
          : t('sftp.password.required'),
  );

  async function save() {
    if (!value || busy) return;
    busy = true;
    err = null;
    note = null;
    try {
      await api.sftpSetPassword(serverId, value);
      value = '';
      onsaved();
    } catch (e) {
      err = isAppError(e) && e.kind === 'sftp_auth' ? t('sftp.password.rejected') : humanizeError(e);
    } finally {
      busy = false;
    }
  }

  async function forget() {
    err = null;
    try {
      await api.sftpForgetPassword(serverId);
      stored = false;
      note = t('sftp.password.forgotten');
    } catch (e) {
      err = humanizeError(e);
    }
  }
</script>

<div class="overlay">
  <form class="modal card" onsubmit={(e) => { e.preventDefault(); void save(); }}>
    <h3>{t('sftp.password.title')}</h3>
    <p class="intro" class:warn={reason === 'rejected'}>{intro}</p>
    <p class="hint">{t('sftp.password.where')}</p>
    <div class="field">
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="input"
        type={show ? 'text' : 'password'}
        bind:value
        placeholder={t('sftp.password.placeholder')}
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        autofocus
        disabled={busy}
      />
      <button type="button" class="btn btn--ghost reveal" onclick={() => (show = !show)}>
        {show ? t('sftp.password.hide') : t('sftp.password.show')}
      </button>
    </div>
    <p class="hint">{t('sftp.password.stored')}</p>
    {#if err}<p class="msg err">{err}</p>{/if}
    {#if note}<p class="msg">{note}</p>{/if}
    <div class="actions">
      {#if stored}
        <button type="button" class="btn btn--ghost" onclick={forget} disabled={busy}>{t('sftp.password.forget')}</button>
      {/if}
      <span class="grow"></span>
      <button type="button" class="btn btn--ghost" onclick={onclose} disabled={busy}>{t('common.cancel')}</button>
      <button type="submit" class="btn" disabled={!value || busy}>
        {busy ? t('sftp.password.checking') : t('sftp.password.save')}
      </button>
    </div>
  </form>
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    background: color-mix(in srgb, #000 45%, transparent);
    z-index: 13;
    padding: 20px;
    display: grid;
    place-items: center;
  }
  .modal {
    padding: 22px;
    width: 100%;
    max-width: 440px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: 16px;
  }
  .intro {
    margin: 0;
    font-size: 13.5px;
  }
  .intro.warn {
    color: var(--state-danger);
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-muted);
  }
  .field {
    display: flex;
    gap: 8px;
  }
  .field .input {
    flex: 1;
    min-width: 0;
  }
  .reveal {
    flex: none;
  }
  .msg {
    margin: 0;
    font-size: 13px;
  }
  .msg.err {
    color: var(--state-danger);
  }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .grow {
    flex: 1;
  }
</style>
