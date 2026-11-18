<script lang="ts">
  /**
   * Changement d'identité du serveur SFTP : le cœur a refusé la connexion parce que l'empreinte de
   * la clé d'hôte n'est plus celle mémorisée (le mot de passe n'a pas été envoyé). L'utilisateur
   * compare les empreintes et décide. Seule l'empreinte réellement présentée par le serveur peut
   * être acceptée : le cœur refuse toute autre valeur.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { SftpHostKey } from '#lib/types.ts';
  import Masked from '../Masked.svelte';

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

<div class="overlay">
  <div class="modal card" role="alertdialog" aria-labelledby="hk-title">
    <h3 id="hk-title">{t('sftp.hostKey.title')}</h3>
    <p class="intro">{t('sftp.hostKey.intro')}</p>
    <p class="hint">{t('sftp.hostKey.causes')}</p>
    {#if info}
      <dl>
        <dt>{t('sftp.hostKey.host')}</dt>
        <dd class="mono"><Masked value={info.host} kind="address" /></dd>
        <dt>{t('sftp.hostKey.known')}</dt>
        <dd class="mono">{info.known_fingerprint ?? '—'}{#if info.known_algorithm}<span class="algo">{info.known_algorithm}</span>{/if}</dd>
        <dt>{t('sftp.hostKey.presented')}</dt>
        <dd class="mono new">
          {info.presented_fingerprint ?? t('sftp.hostKey.none')}{#if info.presented_algorithm}<span class="algo">{info.presented_algorithm}</span>{/if}
        </dd>
      </dl>
    {/if}
    {#if err}<p class="msg err">{err}</p>{/if}
    <div class="actions">
      <button type="button" class="btn btn--ghost" onclick={onclose} disabled={busy}>{t('common.cancel')}</button>
      <button
        type="button"
        class="btn btn--danger"
        onclick={trust}
        disabled={busy || !info?.presented_fingerprint}
      >
        {busy ? t('sftp.hostKey.trusting') : t('sftp.hostKey.trust')}
      </button>
    </div>
  </div>
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
    max-width: 520px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: 16px;
    color: var(--state-danger);
  }
  .intro {
    margin: 0;
    font-size: 13.5px;
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-muted);
  }
  dl {
    margin: 0;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 12px;
    font-size: 12.5px;
  }
  dt {
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .mono {
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .new {
    color: var(--state-danger);
  }
  .algo {
    margin-left: 8px;
    color: var(--text-muted);
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
    justify-content: flex-end;
    gap: 8px;
  }
</style>
