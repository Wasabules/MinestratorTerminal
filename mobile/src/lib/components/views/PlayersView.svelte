<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import type { LiveLight, PlayerAction } from "../../types";

  let { serverId }: { serverId: number } = $props();

  let live = $state<LiveLight | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state<string | null>(null); // clé "player:action" en cours
  let pseudo = $state("");

  async function refresh() {
    try {
      live = await api.liveLight(serverId);
      error = null;
    } catch (err) {
      error = humanizeError(err);
    }
  }

  async function act(player: string, action: PlayerAction) {
    const name = player.trim();
    if (name === "" || busy) return;
    busy = `${name}:${action}`;
    notice = null;
    try {
      await api.playerAction(serverId, action, name);
      notice = t("players.done");
      setTimeout(refresh, 800);
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = null;
    }
  }

  // Actions disponibles pour un pseudo saisi (marche même hors-ligne).
  const byNameActions: { action: PlayerAction; key: string }[] = [
    { action: "ban", key: "action.ban" },
    { action: "unban", key: "action.unban" },
    { action: "op_add", key: "action.op" },
    { action: "op_remove", key: "action.deop" },
    { action: "whitelist_add", key: "action.wl_add" },
    { action: "whitelist_remove", key: "action.wl_remove" },
  ];

  $effect(() => {
    refresh();
    const id = setInterval(refresh, 10000);
    return () => clearInterval(id);
  });
</script>

<div class="view">
  {#if error}<p class="err selectable">{error}</p>{/if}
  {#if notice}<p class="ok">{notice}</p>{/if}

  <!-- Joueurs en ligne -->
  <section>
    <h3>
      {t("players.online")}
      {#if live?.players}<span class="dim">· {live.players.current}/{live.players.limit}</span>{/if}
    </h3>

    {#if live?.players && live.players.list.length > 0}
      <ul>
        {#each live.players.list as name (name)}
          <li class="player">
            <span class="pname selectable">{name}</span>
            <div class="acts">
              <button disabled={busy !== null} onclick={() => act(name, "kick")}>{t("action.kick")}</button>
              <button class="danger" disabled={busy !== null} onclick={() => act(name, "ban")}>{t("action.ban")}</button>
              <button disabled={busy !== null} onclick={() => act(name, "op_add")}>{t("action.op")}</button>
            </div>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="dim">{t("players.none")}</p>
    {/if}
  </section>

  <!-- Action sur un pseudo arbitraire -->
  <section>
    <h3>{t("players.byName")}</h3>
    <input
      class="selectable"
      type="text"
      autocapitalize="off"
      autocomplete="off"
      spellcheck="false"
      placeholder={t("players.pseudo")}
      bind:value={pseudo}
    />
    <div class="grid">
      {#each byNameActions as b (b.action)}
        <button
          class:danger={b.action === "ban"}
          disabled={busy !== null || pseudo.trim() === ""}
          onclick={() => act(pseudo, b.action)}
        >
          {t(b.key)}
        </button>
      {/each}
    </div>
  </section>
</div>

<style>
  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 14px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .player {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
  }
  .pname {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acts {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .acts button {
    padding: 8px 10px;
    font-size: 13px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--text);
    min-height: 38px;
  }
  input {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    padding: 12px;
    font-size: 16px;
    outline: none;
  }
  input:focus {
    border-color: var(--brand-primary);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .grid button {
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    font-weight: 600;
  }
  button.danger {
    color: var(--state-danger);
  }
  button:disabled {
    opacity: 0.45;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
  }
  .ok {
    margin: 0;
    color: var(--brand-primary);
    font-size: 14px;
  }
  .dim {
    color: var(--text-dim);
  }
</style>
