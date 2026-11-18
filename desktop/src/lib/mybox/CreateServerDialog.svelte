<script lang="ts">
  /**
   * Création d'un serveur : jeu (catalogue avec recherche), champs requis, localisation, CPU et RAM
   * bornés à ce qui reste dans la MyBox choisie. « Où le placer ? » demande au cœur un plan de
   * placement (calcul pur) et sélectionne la MyBox proposée. La validation qui fait foi est en Rust.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { CatalogEgg, CreatedServer, MyBoxCapacity, ServerCatalog } from '#lib/types.ts';
  import Modal from './Modal.svelte';
  import { isSecretField } from './util.ts';

  let {
    caps,
    myboxId,
    onclose,
    oncreated,
  }: {
    caps: MyBoxCapacity[];
    myboxId: number;
    onclose: () => void;
    oncreated: (c: CreatedServer) => void;
  } = $props();

  let catalog = $state<ServerCatalog | null>(null);
  let loadError = $state<string | null>(null);
  let selectedMybox = $state(0);
  let query = $state('');
  let withExperimental = $state(false);
  let egg = $state<CatalogEgg | null>(null);
  let name = $state('');
  let nameTouched = $state(false);
  let locationId = $state<number | null>(null);
  let cpu = $state(1);
  let ram = $state(1);
  let fields = $state<Record<string, string>>({});
  let busy = $state(false);
  let error = $state<string | null>(null);
  let hint = $state<string | null>(null);

  onMount(async () => {
    selectedMybox = myboxId;
    try {
      catalog = await api.serverCatalog();
    } catch (e) {
      loadError = humanizeError(e);
    }
  });

  const cap = $derived(caps.find((c) => c.id === selectedMybox) ?? null);
  /** Plafonds des curseurs : le reste de la MyBox, ou deux fois la recommandation pour une Pro. */
  const maxCpu = $derived(cap ? (cap.pro ? Math.max(egg?.cpu ?? 0, 1) * 2 : cap.cpu.remaining) : 0);
  const maxRam = $derived(cap ? (cap.pro ? Math.max(egg?.ram ?? 0, 4) * 2 : cap.ram.remaining) : 0);

  const eggs = $derived.by(() => {
    if (!catalog) return [];
    const q = query.trim().toLowerCase();
    return catalog.eggs
      .filter((e) => (withExperimental || !e.experimental) && (!q || e.name.toLowerCase().includes(q)))
      .sort((a, b) => Number(a.experimental) - Number(b.experimental) || b.certification - a.certification)
      .slice(0, 60);
  });

  const locations = $derived(
    (catalog?.locations ?? []).filter((l) => l.in_stock && (!cap?.free || l.city.toLowerCase() === 'toulouse'))
  );

  function pick(e: CatalogEgg) {
    egg = e;
    fields = Object.fromEntries(e.required_fields.map((f) => [f.key, '']));
    if (!nameTouched) name = t('mybox.defaultName', { game: e.name });
    applyRecommended();
  }

  /** Recommandation du jeu, bornée au reste (1 cœur et 2 Go au minimum, comme le panel). */
  function applyRecommended() {
    cpu = Math.max(1, Math.min(egg?.cpu || 1, maxCpu || 1));
    ram = Math.max(1, Math.min(egg?.ram || 2, maxRam || 1));
  }

  $effect(() => {
    // Localisation par défaut (ou devenue indisponible après un changement de MyBox) : la première
    // disponible, Toulouse en tête si présente.
    if (locations.length > 0 && !locations.some((l) => l.id === locationId)) {
      locationId = (locations.find((l) => l.city.toLowerCase() === 'toulouse') ?? locations[0]).id;
    }
  });

  const missingField = $derived(egg ? egg.required_fields.find((f) => !fields[f.key]?.trim()) : undefined);
  const blocked = $derived(cap ? cap.blockers.filter((b) => b !== 'no_cpu' && b !== 'no_ram') : []);
  const canSubmit = $derived(
    !!cap &&
      !!egg &&
      !busy &&
      blocked.length === 0 &&
      name.trim().length > 0 &&
      name.trim().length <= 50 &&
      !missingField &&
      cpu >= 1 &&
      ram >= 1 &&
      cpu <= maxCpu &&
      ram <= maxRam &&
      (cap.pro || locationId !== null)
  );

  async function suggest() {
    if (!egg) return;
    hint = null;
    try {
      const plan = await api.myboxPlanPlacement(
        [{ label: name.trim() || egg.name, cpu, ram, disk_gb: egg.install_disk }],
        'pack'
      );
      const choice = plan.choices[0];
      if (choice?.mybox_id != null) {
        selectedMybox = choice.mybox_id;
        hint = t('mybox.create.suggested', { name: choice.mybox_name ?? '' });
      } else {
        hint = t('mybox.create.noFit');
      }
    } catch (e) {
      hint = humanizeError(e);
    }
  }

  async function submit() {
    if (!canSubmit || !cap || !egg) return;
    busy = true;
    error = null;
    try {
      const created = await api.myboxCreateServer({
        mybox_id: cap.id,
        name: name.trim(),
        egg_id: egg.id,
        experimental: egg.experimental,
        location_id: cap.pro ? null : locationId,
        cpu,
        ram,
        fields: Object.entries(fields).map(([key, value]) => ({ key, value })),
      });
      oncreated(created);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={t('mybox.create.title')} {onclose} width={640}>
  {#if loadError}
    <p class="alert">{loadError}</p>
  {:else if !catalog}
    <p class="dim"><span class="spinner"></span> {t('common.loading')}</p>
  {:else}
    <label class="field">
      <span class="label">{t('mybox.create.mybox')}</span>
      <select class="input" bind:value={selectedMybox}>
        {#each caps as c (c.id)}
          <option value={c.id}>
            {c.name} — {c.pro
              ? t('mybox.pro')
              : t('mybox.create.free', { cpu: c.cpu.remaining, ram: c.ram.remaining })}
          </option>
        {/each}
      </select>
    </label>
    {#if blocked.length > 0}
      <p class="alert">{blocked.map((b) => t(`mybox.blockers.${b}`)).join(' · ')}</p>
    {/if}

    <div class="field">
      <span class="label">{t('mybox.create.game')}</span>
      <div class="row">
        <input class="input" placeholder={t('mybox.create.search')} bind:value={query} />
        <label class="check">
          <input type="checkbox" bind:checked={withExperimental} />
          {t('mybox.create.experimental')}
        </label>
      </div>
      <div class="eggs">
        {#each eggs as e (`${e.experimental}-${e.id}`)}
          <button class="egg" class:on={egg?.id === e.id && egg?.experimental === e.experimental} onclick={() => pick(e)}>
            {#if e.icon_url}<img src={e.icon_url} alt="" loading="lazy" />{/if}
            <span class="egg-name">{e.name}</span>
            {#if e.experimental}<span class="badge">{t('mybox.create.experimentalBadge')}</span>
            {:else if e.category === 'custom'}<span class="badge">PimpMyBox</span>{/if}
          </button>
        {:else}
          <p class="dim">{t('mybox.create.noGame')}</p>
        {/each}
      </div>
      {#if egg?.experimental}<p class="warn">{t('mybox.create.experimentalWarning')}</p>{/if}
    </div>

    {#if egg}
      <label class="field">
        <span class="label">{t('mybox.create.name')}</span>
        <input class="input" maxlength="50" bind:value={name} oninput={() => (nameTouched = true)} />
      </label>

      {#each egg.required_fields as f (f.key)}
        <label class="field">
          <span class="label">{f.label} <code>{f.key}</code></span>
          <input
            class="input"
            type={isSecretField(f.key) ? 'password' : 'text'}
            autocomplete="off"
            bind:value={fields[f.key]}
          />
          {#if f.description}<span class="dim small">{f.description}</span>{/if}
        </label>
      {/each}

      {#if cap && !cap.pro}
        <label class="field">
          <span class="label">{t('mybox.create.location')}</span>
          <select class="input" bind:value={locationId}>
            {#each locations as l (l.id)}
              <option value={l.id}>{l.city} ({l.country})</option>
            {/each}
          </select>
        </label>
      {/if}

      <p class="dim small">
        {t('mybox.create.recommended', { cpu: egg.cpu || '—', ram: egg.ram || '—', disk: egg.install_disk || '—' })}
      </p>
      {#if maxCpu < 1 || maxRam < 1}
        <p class="alert">{t('mybox.create.noResources')}</p>
      {:else}
        <label class="field">
          <span class="label">{t('mybox.cpuLabel', { n: cpu })}</span>
          <input type="range" min="1" max={maxCpu} step="1" bind:value={cpu} />
        </label>
        <label class="field">
          <span class="label">{t('mybox.ramLabel', { n: ram })}</span>
          <input type="range" min="1" max={maxRam} step="1" bind:value={ram} />
        </label>
        {#if (egg.cpu > 0 && cpu < egg.cpu) || (egg.ram > 0 && ram < egg.ram)}
          <p class="warn">{t('mybox.create.belowRecommended')}</p>
        {/if}
      {/if}
    {/if}

    {#if hint}<p class="dim">{hint}</p>{/if}
    {#if error}<p class="alert">{error}</p>{/if}

    <div class="actions">
      <button class="btn btn--ghost" onclick={suggest} disabled={!egg || busy}>{t('mybox.create.where')}</button>
      <span class="spacer"></span>
      <button class="btn btn--ghost" onclick={onclose}>{t('common.cancel')}</button>
      <button class="btn" onclick={submit} disabled={!canSubmit}>
        {#if busy}<span class="spinner"></span>{/if}
        {t('mybox.create.submit')}
      </button>
    </div>
  {/if}
</Modal>

<style>
  .row {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .check {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 12.5px;
    white-space: nowrap;
    color: var(--text-muted);
  }
  .eggs {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 6px;
    max-height: 210px;
    overflow-y: auto;
    margin-top: 8px;
  }
  .egg {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
    text-align: left;
    font-size: 12.5px;
  }
  .egg.on {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .egg img {
    width: 22px;
    height: 22px;
    border-radius: 5px;
    object-fit: cover;
  }
  .egg-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .small {
    font-size: 12px;
  }
  .warn {
    margin: 0;
    font-size: 12.5px;
    color: var(--state-pending);
  }
  code {
    font-size: 11px;
    color: var(--text-dim);
  }
  input[type='range'] {
    width: 100%;
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
