<script lang="ts">
  /**
   * Tâches planifiées du panel MineStrator (exécutées par l'hébergeur, même téléphone éteint) :
   * lecture et activation. La création et la modification se font sur le desktop ou le panel web.
   */
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import { settings } from "../../stores/settings.svelte";
  import Icon from "../Icon.svelte";
  import type { PanelCron, PanelSchedule, PanelTask } from "../../types";

  let { serverId, active }: { serverId: number; active: boolean } = $props();

  let schedules = $state<PanelSchedule[] | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<number | null>(null);
  let loading = $state(false);

  // Premier affichage : chargement unique (Rafraîchir relance). Drapeau non réactif : un échec
  // ne doit pas redéclencher l'effet en boucle.
  let requested = false;
  $effect(() => {
    if (active && !requested) {
      requested = true;
      void load();
    }
  });

  async function load() {
    loading = true;
    error = null;
    try {
      schedules = await api.panelSchedules(serverId);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  async function toggle(s: PanelSchedule) {
    busy = s.id;
    error = null;
    try {
      await api.panelScheduleUpdate(serverId, s.id, {
        name: s.name,
        cron: s.cron,
        is_active: !s.is_active,
        only_when_online: s.only_when_online,
      });
      await load();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = null;
    }
  }

  const locale = $derived(settings.locale === "en" ? "en" : "fr");
  const pad = (v: string) => v.padStart(2, "0");
  const isNum = (v: string) => /^\d+$/.test(v);

  function dayNames(dow: string): string | null {
    if (!/^[0-7](,[0-7])*$/.test(dow)) return null;
    return [...new Set(dow.split(",").map((d) => Number(d) % 7))]
      .map((d) =>
        new Intl.DateTimeFormat(locale, { weekday: "long", timeZone: "UTC" }).format(
          new Date(Date.UTC(2024, 0, 7 + d)),
        ),
      )
      .join(", ");
  }

  function describe(c: PanelCron): string {
    const raw = `${c.minute} ${c.hour} ${c.day_of_month} ${c.month} ${c.day_of_week}`;
    const everyDay = c.day_of_month === "*" && c.month === "*";
    if (raw === "* * * * *") return t("schedules.cron.everyMinute");
    const step = c.minute.match(/^\*\/(\d+)$/);
    if (step && c.hour === "*" && everyDay && c.day_of_week === "*") {
      return t("schedules.cron.everyNMinutes").replace("{n}", step[1]);
    }
    if (isNum(c.minute) && c.hour === "*" && everyDay && c.day_of_week === "*") {
      return t("schedules.cron.hourly").replace("{minute}", pad(c.minute));
    }
    if (isNum(c.minute) && isNum(c.hour) && everyDay) {
      const time = `${pad(c.hour)}:${pad(c.minute)}`;
      if (c.day_of_week === "*") return t("schedules.cron.daily").replace("{time}", time);
      const days = dayNames(c.day_of_week);
      if (days) return t("schedules.cron.weekly").replace("{days}", days).replace("{time}", time);
    }
    return t("schedules.cron.custom").replace("{cron}", raw);
  }

  function fmtDate(s: string | null): string {
    if (!s) return "";
    const d = new Date(s.replace(" ", "T"));
    return Number.isNaN(d.getTime())
      ? s
      : d.toLocaleString(locale, { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
  }

  function taskText(task: PanelTask): string {
    const what =
      task.action === "power"
        ? t("schedules.taskPower").replace("{action}", t(`schedules.power.${task.payload}`))
        : t("schedules.taskCommand").replace("{command}", task.payload);
    return task.time_offset > 0
      ? `${what} — ${t("schedules.after").replace("{s}", String(task.time_offset))}`
      : what;
  }
</script>

<div class="wrap">
  <div class="head">
    <p class="intro selectable">{t("schedules.intro")}</p>
    <button class="ic" onclick={load} aria-label={t("servers.refresh")} disabled={loading}>
      <Icon name="refresh" size={18} />
    </button>
  </div>

  {#if error}<p class="err selectable">{error}</p>{/if}

  {#if schedules === null}
    <p class="dim pad">{loading ? "…" : ""}</p>
  {:else if schedules.length === 0}
    <p class="dim pad">{t("schedules.none")}</p>
  {:else}
    <ul>
      {#each schedules as s (s.id)}
        <li class="card" class:off={!s.is_active}>
          <div class="top">
            <Icon name="clock" size={18} />
            <div class="meta">
              <span class="name selectable">
                {s.name}
                {#if s.is_processing}<span class="badge">{t("schedules.running")}</span>{/if}
              </span>
              <span class="dim small">
                {describe(s.cron)}{#if s.only_when_online} · {t("schedules.onlyWhenOnline")}{/if}
              </span>
              <span class="dim small">
                {#if s.is_active && s.next_run_at}
                  {t("schedules.next").replace("{date}", fmtDate(s.next_run_at))}
                {:else if !s.is_active}
                  {t("schedules.inactive")}
                {/if}
                {#if s.last_run_at} · {t("schedules.last").replace("{date}", fmtDate(s.last_run_at))}{/if}
              </span>
            </div>
            <label class="switch" aria-label={t("schedules.active")}>
              <input type="checkbox" checked={s.is_active} disabled={busy === s.id} onchange={() => toggle(s)} />
            </label>
          </div>
          {#if s.tasks.length === 0}
            <p class="dim small task">{t("schedules.noActions")}</p>
          {:else}
            <ol>
              {#each s.tasks as task (task.id)}
                <li class="task selectable">{taskText(task)}</li>
              {/each}
            </ol>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  <p class="dim small pad">{t("schedules.manageElsewhere")}</p>
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px 12px calc(var(--safe-bottom) + 12px);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .intro {
    flex: 1;
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .ic {
    background: transparent;
    border: none;
    color: var(--text);
    min-width: 44px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 12px;
  }
  .card.off .meta {
    opacity: 0.55;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-dim);
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    color: var(--text);
  }
  .name {
    font-weight: 600;
  }
  .badge {
    margin-left: 6px;
    font-size: 12px;
    color: var(--brand-primary);
  }
  .small {
    font-size: 12.5px;
  }
  .switch {
    display: inline-flex;
    align-items: center;
    min-height: 44px;
    padding: 0 6px;
  }
  .switch input {
    width: 22px;
    height: 22px;
  }
  ol {
    margin: 8px 0 0;
    padding-left: 46px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .task {
    font-size: 13px;
    overflow-wrap: anywhere;
  }
  p.task {
    margin: 6px 0 0 28px;
  }
  .dim {
    color: var(--text-dim);
  }
  .pad {
    margin: 0;
    padding: 8px 4px;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
  }
</style>
