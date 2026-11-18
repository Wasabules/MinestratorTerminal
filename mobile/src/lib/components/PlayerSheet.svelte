<script lang="ts">
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import { settings } from "../stores/settings.svelte";
  import { portal } from "../portal";
  import Icon from "./Icon.svelte";
  import { avatarStyle } from "../servers/colors.svelte";
  import type { AppError, InventoryItem, KnownPlayer, PlayerInventory, PlayerProfile } from "../types";

  /**
   * Fiche d'un joueur déjà venu, plein écran, en lecture seule : activité, état, combats et
   * inventaire en liste (emplacement, objet, quantité, enchantements).
   */
  let { serverId, player, onClose }: { serverId: number; player: KnownPlayer; onClose: () => void } = $props();

  let profile = $state<PlayerProfile | null>(null);
  let profileError = $state<string | null>(null);
  let inventory = $state<PlayerInventory | null>(null);
  let inventoryError = $state<string | null>(null);
  let loading = $state(true);

  const locale = $derived(settings.locale === "en" ? "en-GB" : "fr-FR");
  const name = $derived(profile?.name ?? player.name ?? t("players.unknownName"));
  const flags = $derived(profile?.flags ?? player.flags);

  const DATA_ERRORS = ["NOT_FOUND", "VOLUME_UNREACHABLE", "SCRIPT_UNAVAILABLE", "UNSUPPORTED", "TOO_LARGE"];

  /** Erreur de lecture des fichiers joueur (`API_PLAYERDATA_*`) traduite, sinon message générique. */
  function dataError(e: unknown): string {
    const err = e as Partial<AppError> | undefined;
    if (err?.kind === "api" && err.message) {
      const code = DATA_ERRORS.find((c) => err.message?.includes(`PLAYERDATA_${c}`));
      if (code) return t(`players.dataErrors.${code}`);
    }
    return humanizeError(e);
  }

  $effect(() => {
    void load(player.uuid);
  });

  async function load(uuid: string) {
    loading = true;
    const [p, inv] = await Promise.allSettled([api.playerProfile(serverId, uuid), api.playerInventory(serverId, uuid)]);
    if (p.status === "fulfilled") profile = p.value;
    else profileError = dataError(p.reason);
    if (inv.status === "fulfilled") inventory = inv.value;
    else inventoryError = dataError(inv.reason);
    loading = false;
  }

  function playTime(secs: number | null): string {
    if (secs === null) return "—";
    const m = Math.floor(secs / 60);
    return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, "0")}`;
  }

  function date(ts: number | null): string {
    if (ts === null) return "—";
    return new Date(ts * 1000).toLocaleString(locale, {
      day: "2-digit",
      month: "2-digit",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  function roman(n: number): string {
    const table: [number, string][] = [
      [10, "X"],
      [9, "IX"],
      [5, "V"],
      [4, "IV"],
      [1, "I"],
    ];
    if (n > 30) return String(n);
    let out = "";
    let rest = n;
    for (const [v, s] of table) {
      while (rest >= v) {
        out += s;
        rest -= v;
      }
    }
    return out;
  }

  type Row = [string, string];

  const activity = $derived.by<Row[]>(() => {
    const p = profile;
    const rows: Row[] = [[t("players.playTime"), playTime(p?.play_time_secs ?? player.play_time_secs)]];
    if (p?.sessions != null) rows.push([t("players.sheet.sessions"), String(p.sessions)]);
    if (p?.first_played != null) rows.push([t("players.sheet.firstPlayed"), date(p.first_played)]);
    rows.push([t("players.lastSeen"), date(p?.last_seen ?? player.last_seen)]);
    return rows;
  });

  const condition = $derived.by<Row[]>(() => {
    const p = profile;
    if (!p) return [];
    const rows: Row[] = [];
    if (p.game_mode) rows.push([t("players.sheet.gameMode"), t(`players.gameMode.${p.game_mode}`)]);
    if (p.health != null) rows.push([t("players.sheet.health"), `${p.health / 2} / ${(p.max_health ?? 20) / 2} ❤`]);
    if (p.food_level != null) rows.push([t("players.sheet.food"), `${p.food_level} / 20`]);
    if (p.xp_level != null) rows.push([t("players.sheet.xpLevel"), String(p.xp_level)]);
    if (p.deaths != null) rows.push([t("players.sheet.deaths"), String(p.deaths)]);
    if (p.mob_kills != null) rows.push([t("players.sheet.mobKills"), String(p.mob_kills)]);
    if (p.player_kills != null) rows.push([t("players.sheet.playerKills"), String(p.player_kills)]);
    return rows;
  });

  const ARMOR = ["helmet", "chestplate", "leggings", "boots"];

  /** Inventaire à plat : un objet par ligne, avec le nom de son emplacement. */
  const items = $derived.by<{ slot: string; item: InventoryItem }[]>(() => {
    const inv = inventory;
    if (!inv) return [];
    const out: { slot: string; item: InventoryItem }[] = [];
    inv.armor.forEach((item, i) => item && out.push({ slot: t(`players.armorSlot.${ARMOR[i]}`), item }));
    if (inv.offhand) out.push({ slot: t("players.sheet.offhand"), item: inv.offhand });
    inv.hotbar.forEach((item, i) => item && out.push({ slot: `${t("players.sheet.hotbar")} ${i + 1}`, item }));
    inv.main.forEach((item, i) => item && out.push({ slot: `${t("players.sheet.bag")} ${i + 1}`, item }));
    return out;
  });
</script>

<div class="psheet" use:portal role="dialog" aria-modal="true" aria-label={name}>
  <header>
    <button class="ic" aria-label={t("players.sheet.close")} onclick={onClose}><Icon name="back" size={22} /></button>
    <span class="avatar" style={avatarStyle(name)} aria-hidden="true">{name.charAt(0).toUpperCase()}</span>
    <div class="who">
      <span class="title selectable">{name}</span>
      <span class="uuid selectable">{player.uuid}</span>
    </div>
  </header>

  <div class="body">
    <div class="badges">
      {#if player.online}<span class="tag on">{t("players.badge.online")}</span>{/if}
      {#if flags.op}<span class="tag op">{t("players.badge.op")}</span>{/if}
      {#if flags.whitelisted}<span class="tag wl">{t("players.badge.whitelisted")}</span>{/if}
      {#if flags.banned}<span class="tag ban">{t("players.badge.banned")}</span>{/if}
    </div>

    {#if loading}
      <p class="dim">…</p>
    {:else}
      <section>
        <h3>{t("players.sheet.activity")}</h3>
        <dl>
          {#each activity as [k, v] (k)}<dt>{k}</dt><dd>{v}</dd>{/each}
        </dl>
        {#if profileError}<p class="err">{profileError}</p>{/if}
      </section>

      {#if condition.length > 0}
        <section>
          <h3>{t("players.sheet.state")}</h3>
          <dl>
            {#each condition as [k, v] (k)}<dt>{k}</dt><dd>{v}</dd>{/each}
          </dl>
        </section>
      {/if}

      <section>
        <h3>{t("players.sheet.inventory")}</h3>
        {#if inventoryError}
          <p class="err">{inventoryError}</p>
        {:else if inventory}
          {#if inventory.online}<p class="warn">{t("players.sheet.onlineWarn")}</p>{/if}
          {#if items.length === 0}
            <p class="dim">{t("players.sheet.emptyInventory")}</p>
          {:else}
            <ul>
              {#each items as { slot, item }, i (i)}
                <li>
                  {#if item.icon_url}<img src={item.icon_url} alt="" loading="lazy" />{:else}<span class="noic"></span>{/if}
                  <div class="it">
                    <span class="iname">{item.name}{item.count > 1 ? ` ×${item.count}` : ""}</span>
                    {#if item.enchantments.length > 0}
                      <span class="ench">{item.enchantments.map((e) => `${e.name} ${roman(e.level)}`).join(", ")}</span>
                    {/if}
                  </div>
                  <span class="slot dim">{slot}</span>
                </li>
              {/each}
            </ul>
          {/if}
          <p class="dim small">
            {#if inventory.saved_at}{t("players.sheet.savedAt").replace("{date}", date(inventory.saved_at))} · {/if}{t(
              "players.sheet.readOnly",
            )}
          </p>
        {/if}
      </section>
    {/if}
  </div>
</div>

<style>
  .psheet {
    position: fixed;
    inset: 0;
    z-index: 45;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: calc(var(--safe-top) + 8px) 12px 8px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text);
    flex: none;
  }
  /* Initiale sur l'accent d'un biome tiré du pseudo (encre sombre, lisible dans les deux thèmes). */
  .avatar {
    flex: none;
    width: 36px;
    height: 36px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    font-weight: 900;
    font-size: 16px;
  }
  .who {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .title {
    font-size: 18px;
    font-weight: 900;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .uuid {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .body {
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding: 14px 16px calc(var(--safe-bottom) + 24px);
  }
  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .badges:empty {
    display: none;
  }
  .tag {
    font-size: 12px;
    font-weight: 800;
    padding: 2px 9px;
    border-radius: 999px;
    background: color-mix(in srgb, currentColor 14%, transparent);
  }
  .tag.on {
    color: var(--state-running-text);
  }
  .tag.op {
    color: var(--state-hibernate);
  }
  .tag.wl {
    color: var(--state-pending);
  }
  .tag.ban {
    color: var(--state-danger);
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 14px;
    margin: 0;
    padding: 14px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    font-size: 14px;
  }
  dt {
    color: var(--text-dim);
  }
  dd {
    margin: 0;
    text-align: right;
    font-weight: 800;
    font-variant-numeric: tabular-nums;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  li:last-child {
    border-bottom: none;
  }
  img,
  .noic {
    width: 28px;
    height: 28px;
    flex: none;
    image-rendering: pixelated;
  }
  .it {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .iname {
    font-weight: 800;
    font-size: 14px;
  }
  .ench {
    font-size: 12px;
    color: color-mix(in srgb, var(--state-hibernate) 55%, var(--text));
  }
  .slot {
    font-size: 12px;
    text-align: right;
    flex: none;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
  }
  .warn {
    margin: 0;
    color: var(--state-pending);
    font-size: 13px;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
</style>
