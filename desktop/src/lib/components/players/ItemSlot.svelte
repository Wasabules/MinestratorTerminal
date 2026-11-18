<script lang="ts">
  import { t } from '#lib/i18n/index.ts';
  import type { InventoryItem } from '#lib/types.ts';

  /** Un emplacement d'inventaire : icône, quantité, usure, enchantement (lecture seule). */
  let { item, label = '' }: { item: InventoryItem | null; label?: string } = $props();

  let iconFailed = $state(false);

  /** Infobulle : nom, quantité, enchantements, usure, puis l'emplacement. */
  const tip = $derived.by(() => {
    if (!item) return label;
    const lines = [item.count > 1 ? `${item.name} ×${item.count}` : item.name];
    if (item.custom_name) lines.push(item.id);
    for (const e of item.enchantments) lines.push(`${e.name} ${roman(e.level)}`);
    if (item.durability !== null) {
      lines.push(t('players.sheet.durability', { n: Math.round(item.durability * 100) }));
    }
    if (label) lines.push(`— ${label}`);
    return lines.join('\n');
  });

  /** Initiales affichées quand l'icône manque (objet de mod, CDN injoignable). */
  const initials = $derived(
    item
      ? item.name
          .split(/\s+/)
          .slice(0, 2)
          .map((w) => w.charAt(0).toUpperCase())
          .join('')
      : '',
  );

  function roman(n: number): string {
    const table: [number, string][] = [
      [10, 'X'],
      [9, 'IX'],
      [5, 'V'],
      [4, 'IV'],
      [1, 'I'],
    ];
    if (n > 30) return String(n);
    let out = '';
    let rest = n;
    for (const [v, s] of table) {
      while (rest >= v) {
        out += s;
        rest -= v;
      }
    }
    return out;
  }

  // Un autre objet dans le même emplacement : on retente l'icône.
  $effect(() => {
    void item?.icon_url;
    iconFailed = false;
  });
</script>

<div class="slot" class:filled={item !== null} class:ench={item !== null && item.enchantments.length > 0} title={tip}>
  {#if item}
    {#if item.icon_url && !iconFailed}
      <img src={item.icon_url} alt={item.name} loading="lazy" onerror={() => (iconFailed = true)} />
    {:else}
      <span class="ini">{initials}</span>
    {/if}
    {#if item.count > 1}<span class="count">{item.count}</span>{/if}
    {#if item.durability !== null}
      <span class="dura"
        ><span
          style={`width: ${Math.round(item.durability * 100)}%; background: var(${
            item.durability > 0.5 ? '--state-running' : item.durability > 0.2 ? '--state-pending' : '--state-danger'
          })`}
        ></span></span
      >
    {/if}
  {/if}
</div>

<style>
  .slot {
    position: relative;
    width: 40px;
    height: 40px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--text) 5%, var(--bg));
    border: 1px solid var(--border);
    box-shadow: inset 0 2px 4px rgba(0, 0, 0, 0.25);
    display: grid;
    place-items: center;
    flex: none;
  }
  .slot.ench {
    border-color: color-mix(in srgb, var(--brand-tertiary) 70%, var(--border));
    box-shadow:
      inset 0 2px 4px rgba(0, 0, 0, 0.25),
      0 0 0 1px color-mix(in srgb, var(--brand-tertiary) 35%, transparent);
  }
  img {
    width: 30px;
    height: 30px;
    image-rendering: pixelated;
  }
  .ini {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 700;
    color: var(--text-muted);
  }
  .count {
    position: absolute;
    right: 3px;
    bottom: 1px;
    font-size: 11px;
    font-weight: 700;
    color: #fff;
    text-shadow: 0 1px 1px rgba(0, 0, 0, 0.9);
  }
  .dura {
    position: absolute;
    left: 4px;
    right: 4px;
    bottom: 3px;
    height: 3px;
    border-radius: 2px;
    background: rgba(0, 0, 0, 0.55);
    overflow: hidden;
  }
  .dura span {
    display: block;
    height: 100%;
  }
</style>
