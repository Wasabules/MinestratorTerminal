<script lang="ts">
  /**
   * Gestion des MyBox : jauges de ressources (alloué / total, disque partagé, emplacements),
   * serveurs de chaque MyBox avec redimensionnement, pause/reprise, renommage, ordre et
   * suppression. Toute la logique (validations, corps envoyés) est dans `core/src/mybox.rs`.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { MyBoxCapacity, MyBoxServerSlot } from '#lib/types.ts';
  import Gauge from '../Gauge.svelte';
  import Icon from '../Icon.svelte';
  import Modal from '#lib/mybox/Modal.svelte';
  import CreateServerDialog from '#lib/mybox/CreateServerDialog.svelte';
  import ResizeDialog from '#lib/mybox/ResizeDialog.svelte';
  import DeleteServerDialog from '#lib/mybox/DeleteServerDialog.svelte';
  import { percent } from '#lib/mybox/util.ts';

  let caps = $state<MyBoxCapacity[] | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state<string | null>(null);

  /** Dialogue ouvert (un seul à la fois). */
  let creating = $state<number | null>(null);
  let resizing = $state<{ cap: MyBoxCapacity; server: MyBoxServerSlot } | null>(null);
  let deleting = $state<{ cap: MyBoxCapacity; server: MyBoxServerSlot } | null>(null);
  let pausing = $state<{ cap: MyBoxCapacity; server: MyBoxServerSlot } | null>(null);
  /** Renommage en place : `mybox:<id>` ou `server:<id>`. */
  let renaming = $state<string | null>(null);
  let renameValue = $state('');

  async function load() {
    loading = true;
    error = null;
    try {
      caps = await api.myboxCapacities();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  onMount(load);

  /** Exécute une écriture, affiche son résultat, puis relit les capacités. */
  async function run(key: string, action: () => Promise<unknown>, success: string | (() => string)) {
    busy = key;
    notice = null;
    try {
      await action();
      notice = typeof success === 'string' ? success : success();
      await load();
    } catch (e) {
      notice = humanizeError(e);
    } finally {
      busy = null;
    }
  }

  function startRename(key: string, current: string) {
    renaming = key;
    renameValue = current;
  }

  async function commitRename(kind: 'mybox' | 'server', id: number) {
    const name = renameValue.trim();
    renaming = null;
    if (!name) return;
    await run(
      `${kind}:${id}`,
      () => (kind === 'mybox' ? api.renameMybox(id, name) : api.renameServer(id, name)),
      t('mybox.renamed')
    );
  }

  function renameKeys(e: KeyboardEvent, kind: 'mybox' | 'server', id: number) {
    if (e.key === 'Enter') void commitRename(kind, id);
    else if (e.key === 'Escape') renaming = null;
  }

  /** Déplace un serveur d'un cran parmi ceux de même état (actifs ou en pause). */
  async function move(cap: MyBoxCapacity, server: MyBoxServerSlot, delta: number) {
    const ids = cap.servers.map((s) => s.id);
    const i = ids.indexOf(server.id);
    const j = i + delta;
    if (j < 0 || j >= ids.length || cap.servers[j].paused !== server.paused) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    await run(`order:${cap.id}`, () => api.myboxReorderServers(cap.id, ids), t('mybox.orderSaved'));
  }

  async function confirmPause() {
    if (!pausing) return;
    const { cap, server } = pausing;
    pausing = null;
    let resuming = false;
    await run(
      `server:${server.id}`,
      async () => {
        resuming = (await api.myboxSetServerPaused(cap.id, server.id, !server.paused)).resuming;
      },
      () => (resuming ? t('mybox.resuming') : server.paused ? t('mybox.resumed') : t('mybox.paused'))
    );
  }

  function diskDetail(c: MyBoxCapacity): string {
    return c.disk.used_mb == null
      ? t('mybox.diskTotal', { total: c.disk.total_gb })
      : t('mybox.diskUsed', { used: (c.disk.used_mb / 1000).toFixed(1), total: c.disk.total_gb });
  }
</script>

<div class="mb">
  <header class="head">
    <div>
      <h1>{t('mybox.title')}</h1>
      <p class="dim">{t('mybox.subtitle')}</p>
    </div>
    <button class="btn btn--ghost" onclick={load} disabled={loading}>
      {#if loading}<span class="spinner"></span>{/if}
      {t('common.refresh')}
    </button>
  </header>

  {#if notice}<p class="notice card">{notice}</p>{/if}

  {#if loading && !caps}
    <div class="state"><span class="spinner"></span> {t('common.loading')}</div>
  {:else if error}
    <div class="state">
      <p class="alert">{error}</p>
      <button class="btn" onclick={load}>{t('common.retry')}</button>
    </div>
  {:else if caps}
    {#each caps as cap (cap.id)}
      <section class="box card">
        <div class="box-head">
          <div class="titles">
            {#if renaming === `mybox:${cap.id}`}
              <!-- svelte-ignore a11y_autofocus -->
              <input
                class="input rename"
                maxlength="50"
                autofocus
                bind:value={renameValue}
                onkeydown={(e) => renameKeys(e, 'mybox', cap.id)}
                onblur={() => commitRename('mybox', cap.id)}
              />
            {:else}
              <h2>{cap.name}</h2>
              {#if cap.owner}
                <button class="icon-btn" title={t('mybox.rename')} onclick={() => startRename(`mybox:${cap.id}`, cap.name)}>
                  <Icon name="pencil" size={14} />
                </button>
              {/if}
            {/if}
            <span class="dim">{cap.offer}</span>
            {#if cap.pro}<span class="badge">{t('mybox.pro')}</span>{/if}
            {#if cap.free}<span class="badge">{t('mybox.freeOffer')}</span>{/if}
            {#if cap.expired}<span class="badge danger">{t('home.expired')}</span>{/if}
            {#if cap.suspended}<span class="badge danger">{t('home.suspended')}</span>{/if}
          </div>
          <button
            class="btn"
            disabled={cap.blockers.length > 0}
            title={cap.blockers.map((b) => t(`mybox.blockers.${b}`)).join(' · ')}
            onclick={() => (creating = cap.id)}
          >
            <Icon name="plus" size={14} />
            {t('mybox.createServer')}
          </button>
        </div>

        {#if cap.pro}
          <p class="dim">{t('mybox.proHint')}</p>
        {:else}
          <div class="gauges">
            <Gauge
              label={t('mybox.cpu')}
              percent={percent(cap.cpu.allocated, cap.cpu.total)}
              detail={t('mybox.cpuDetail', { allocated: cap.cpu.allocated, total: cap.cpu.total, remaining: cap.cpu.remaining })}
            />
            <Gauge
              label={t('mybox.ram')}
              percent={percent(cap.ram.allocated, cap.ram.total)}
              detail={t('mybox.ramDetail', { allocated: cap.ram.allocated, total: cap.ram.total, remaining: cap.ram.remaining })}
            />
            <Gauge
              label={t('mybox.disk')}
              percent={cap.disk.used_mb == null ? 0 : percent(cap.disk.used_mb / 1000, cap.disk.total_gb)}
              detail={diskDetail(cap)}
            />
            <Gauge
              label={t('mybox.slots')}
              percent={percent(cap.slots.active, cap.slots.max_active)}
              detail={t('mybox.slotsDetail', {
                active: cap.slots.active,
                max: cap.slots.max_active,
                paused: cap.slots.paused,
                maxPaused: cap.slots.max_paused,
              })}
            />
          </div>
          {#if cap.flexcore > 0}<p class="dim small">{t('mybox.flexcore', { n: cap.flexcore })}</p>{/if}
        {/if}
        {#if cap.blockers.length > 0 && !cap.expired && !cap.suspended}
          <p class="dim small">{cap.blockers.map((b) => t(`mybox.blockers.${b}`)).join(' · ')}</p>
        {/if}

        {#if cap.servers.length === 0}
          <p class="dim">{t('home.emptyMybox')}</p>
        {:else}
          <table>
            <thead>
              <tr>
                <th></th>
                <th>{t('mybox.col.name')}</th>
                <th>{t('mybox.col.game')}</th>
                <th class="num">{t('mybox.col.cpu')}</th>
                <th class="num">{t('mybox.col.ram')}</th>
                <th>{t('mybox.col.state')}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {#each cap.servers as s, i (s.id)}
                <tr class:paused={s.paused}>
                  <td class="order">
                    <button
                      class="icon-btn"
                      title={t('mybox.moveUp')}
                      disabled={!!busy || i === 0 || cap.servers[i - 1].paused !== s.paused}
                      onclick={() => move(cap, s, -1)}
                    ><Icon name="arrow-up" size={12} /></button>
                    <button
                      class="icon-btn"
                      title={t('mybox.moveDown')}
                      disabled={!!busy || i === cap.servers.length - 1 || cap.servers[i + 1].paused !== s.paused}
                      onclick={() => move(cap, s, 1)}
                    ><Icon name="arrow-down" size={12} /></button>
                  </td>
                  <td>
                    {#if renaming === `server:${s.id}`}
                      <!-- svelte-ignore a11y_autofocus -->
                      <input
                        class="input rename"
                        maxlength="50"
                        autofocus
                        bind:value={renameValue}
                        onkeydown={(e) => renameKeys(e, 'server', s.id)}
                        onblur={() => commitRename('server', s.id)}
                      />
                    {:else}
                      <span class="sname">{s.name}</span>
                      <span class="dim small">{s.location}</span>
                    {/if}
                  </td>
                  <td class="dim">{s.egg_name}</td>
                  <td class="num">{s.cpu}</td>
                  <td class="num">{s.ram}</td>
                  <td>
                    {#if s.expired}<span class="badge danger">{t('mybox.state.expired')}</span>
                    {:else if s.paused}<span class="badge">{t('mybox.state.paused')}</span>
                    {:else}<span class="badge ok">{t('mybox.state.active')}</span>{/if}
                  </td>
                  <td class="acts">
                    {#if busy === `server:${s.id}`}<span class="spinner"></span>{/if}
                    <button class="icon-btn" title={t('mybox.rename')} onclick={() => startRename(`server:${s.id}`, s.name)}>
                      <Icon name="pencil" size={14} />
                    </button>
                    <button class="icon-btn" title={t('mybox.resize.action')} onclick={() => (resizing = { cap, server: s })}>
                      <Icon name="cpu" size={14} />
                    </button>
                    <button
                      class="icon-btn"
                      title={s.paused ? t('mybox.resume') : t('mybox.pause')}
                      onclick={() => (pausing = { cap, server: s })}
                    >
                      <Icon name={s.paused ? 'play' : 'square'} size={14} />
                    </button>
                    <button class="icon-btn danger" title={t('mybox.delete.action')} onclick={() => (deleting = { cap, server: s })}>
                      <Icon name="trash" size={14} />
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>
    {:else}
      <div class="state dim">{t('home.noServers')}</div>
    {/each}
  {/if}
</div>

{#if creating !== null && caps}
  <CreateServerDialog
    {caps}
    myboxId={creating}
    onclose={() => (creating = null)}
    oncreated={(c) => {
      creating = null;
      notice = t('mybox.created', { id: c.server_id });
      void load();
    }}
  />
{/if}

{#if resizing}
  <ResizeDialog
    cap={resizing.cap}
    server={resizing.server}
    onclose={() => (resizing = null)}
    ondone={() => {
      resizing = null;
      notice = t('mybox.resized');
      void load();
    }}
  />
{/if}

{#if deleting}
  <DeleteServerDialog
    cap={deleting.cap}
    server={deleting.server}
    onclose={() => (deleting = null)}
    ondone={() => {
      deleting = null;
      notice = t('mybox.deleted');
      void load();
    }}
  />
{/if}

{#if pausing}
  <Modal title={pausing.server.paused ? t('mybox.resumeTitle', { name: pausing.server.name }) : t('mybox.pauseTitle', { name: pausing.server.name })} onclose={() => (pausing = null)}>
    <p class="dim">{pausing.server.paused ? t('mybox.resumeBody') : t('mybox.pauseBody')}</p>
    <div class="actions">
      <span class="spacer"></span>
      <button class="btn btn--ghost" onclick={() => (pausing = null)}>{t('common.cancel')}</button>
      <button class="btn" onclick={confirmPause}>{pausing.server.paused ? t('mybox.resume') : t('mybox.pause')}</button>
    </div>
  </Modal>
{/if}

<style>
  .mb {
    max-width: 1080px;
    margin: 0 auto;
    padding: 28px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
  }
  .head h1 {
    margin: 0;
    font-size: 24px;
    letter-spacing: -0.02em;
  }
  .head p {
    margin: 4px 0 0;
    font-size: 13px;
  }
  .notice {
    margin: 0;
    padding: 10px 14px;
    font-size: 13px;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 48px 0;
    color: var(--text-muted);
  }
  .box {
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .box-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }
  .titles {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .titles h2 {
    margin: 0;
    font-size: 16px;
  }
  .gauges {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 16px;
  }
  .small {
    font-size: 12px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: 11.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  td {
    padding: 7px 8px;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  tr.paused td {
    opacity: 0.7;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .sname {
    font-weight: 500;
    margin-right: 6px;
  }
  .order {
    width: 52px;
    white-space: nowrap;
  }
  .acts {
    text-align: right;
    white-space: nowrap;
  }
  .icon-btn {
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .icon-btn:hover:not(:disabled) {
    background: var(--elevated);
    color: var(--text);
  }
  .icon-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .icon-btn.danger:hover:not(:disabled) {
    color: var(--state-danger);
  }
  .badge.ok {
    color: var(--state-running);
  }
  .badge.danger {
    color: var(--state-danger);
  }
  .rename {
    max-width: 260px;
    padding: 4px 8px;
  }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .spacer {
    flex: 1;
  }
</style>
