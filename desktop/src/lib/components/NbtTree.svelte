<script lang="ts">
  // Inspecteur NBT repliable (lecture seule), style NBTExplorer : chaque nœud porte son TYPE NBT
  // exact (puce colorée). Fonctions : recherche (surlignage + auto-dépli via arbre pré-filtré),
  // hints « parlants » (UUID/date/coordonnées/booléen), dépli/repli groupé, menu contextuel (copier).
  import { untrack } from 'svelte';
  import type { NbtNode } from '$lib/types';
  import Self from './NbtTree.svelte';

  let {
    node,
    depth = 0,
    path = '',
    query = '',
    bulk = null,
    onContext,
  }: {
    node: NbtNode;
    depth?: number;
    path?: string;
    query?: string;
    bulk?: { gen: number; open: boolean } | null;
    onContext?: (e: MouseEvent, node: NbtNode, path: string) => void;
  } = $props();

  const container = $derived(node.tag === 'Compound' || node.tag === 'List');
  const hasChildren = $derived(container && (node.children?.length ?? 0) > 0);
  // Racine + 1er niveau ouverts ; `depth` constant par instance → capture initiale volontaire.
  let open = $state(untrack(() => depth < 1));
  // Dépli/repli groupé : quand la génération change, on aligne l'état ouvert.
  let bulkGen = -1;
  $effect(() => {
    if (bulk && bulk.gen !== bulkGen) {
      bulkGen = bulk.gen;
      open = bulk.open;
    }
  });
  const expanded = $derived(query.length > 0 ? true : open);

  const TYPE: Record<string, { label: string; color: string }> = {
    Byte: { label: 'byte', color: '#5aa9e6' },
    Short: { label: 'short', color: '#5aa9e6' },
    Int: { label: 'int', color: '#5aa9e6' },
    Long: { label: 'long', color: '#5aa9e6' },
    Float: { label: 'float', color: '#b58cf2' },
    Double: { label: 'double', color: '#b58cf2' },
    String: { label: 'string', color: '#5ecb9e' },
    ByteArray: { label: 'byte[]', color: '#e0a458' },
    IntArray: { label: 'int[]', color: '#e0a458' },
    LongArray: { label: 'long[]', color: '#e0a458' },
    List: { label: 'list', color: '#3fc7c0' },
    Compound: { label: 'compound', color: '#8b9dff' },
  };
  const meta = $derived(TYPE[node.tag] ?? { label: node.tag, color: 'var(--text-dim)' });
  const pad = $derived(depth * 15 + 8);
  const hint = $derived(smartHint(node));

  function childPath(child: NbtNode, i: number): string {
    if (child.name != null) return path ? `${path}.${child.name}` : child.name;
    return `${path}[${i}]`;
  }

  // Découpe un texte pour surligner la sous-chaîne recherchée.
  function parts(text: string): { t: string; hit: boolean }[] {
    if (!query) return [{ t: text, hit: false }];
    const lc = text.toLowerCase();
    const q = query.toLowerCase();
    const out: { t: string; hit: boolean }[] = [];
    let i = 0;
    for (let j = lc.indexOf(q, i); j !== -1; j = lc.indexOf(q, i)) {
      if (j > i) out.push({ t: text.slice(i, j), hit: false });
      out.push({ t: text.slice(j, j + q.length), hit: true });
      i = j + q.length;
    }
    if (i < text.length) out.push({ t: text.slice(i), hit: false });
    return out;
  }

  // --- Hints « parlants » (frontend, sans altérer la valeur brute) ---
  function smartHint(n: NbtNode): string | null {
    if (n.tag === 'IntArray' && n.len === 4 && n.value) {
      const p = n.value.split(',').map((s) => Number(s.trim()));
      if (p.length === 4 && p.every(Number.isFinite)) return intsToUuid(p);
    }
    if (n.tag === 'List' && n.children && n.children.length >= 2 && n.children.length <= 3) {
      if (n.children.every((c) => c.tag === 'Float' || c.tag === 'Double')) {
        return `(${n.children.map((c) => c.value ?? '?').join(', ')})`;
      }
    }
    if (n.tag === 'Long' && n.name && n.value && /played|modified|timestamp|lastsave|created|date/i.test(n.name)) {
      const num = Number(n.value);
      if (Number.isFinite(num) && num > 1e9) {
        const d = new Date(num > 1e12 ? num : num * 1000);
        if (!isNaN(d.getTime())) return d.toLocaleString();
      }
    }
    if (n.tag === 'Byte' && (n.value === '0' || n.value === '1')) {
      if (!/count|slot|damage|data|age|level|size|amount/i.test(n.name ?? '')) {
        return n.value === '1' ? 'vrai' : 'faux';
      }
    }
    return null;
  }
  function intsToUuid(ints: number[]): string {
    const hex = ints.map((n) => (n >>> 0).toString(16).padStart(8, '0')).join('');
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }
</script>

<div class="nbtnode">
  {#if hasChildren}
    <button
      class="nrow"
      style="padding-left: {pad}px"
      onclick={() => (open = !open)}
      oncontextmenu={(e) => onContext?.(e, node, path)}
    >
      <span class="tw">{expanded ? '▾' : '▸'}</span>
      {#if node.name != null}<span class="nkey">{#each parts(node.name) as p}{#if p.hit}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}</span>{/if}
      <span class="ncount">{node.len ?? 0}</span>
      <span class="ntag" style="--tc: {meta.color}">{meta.label}</span>
    </button>
  {:else}
    <div class="nrow leaf" style="padding-left: {pad}px" oncontextmenu={(e) => onContext?.(e, node, path)} role="treeitem" aria-selected="false" tabindex="-1">
      <span class="tw"></span>
      {#if node.name != null}<span class="nkey">{#each parts(node.name) as p}{#if p.hit}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}</span>{/if}
      {#if container}
        <span class="ncount">{node.len ?? 0}</span>
      {:else if node.value != null}
        <span class="nval">{#if node.tag === 'String'}"{/if}{#each parts(node.value) as p}{#if p.hit}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}{#if node.tag === 'String'}"{/if}</span>
      {/if}
      {#if hint}<span class="nhint" title={hint}>→ {hint}</span>{/if}
      <span class="ntag" style="--tc: {meta.color}">{meta.label}</span>
    </div>
  {/if}
  {#if hasChildren && expanded}
    {#each node.children ?? [] as child, i (i)}
      <Self node={child} depth={depth + 1} path={childPath(child, i)} {query} {bulk} {onContext} />
    {/each}
  {/if}
</div>

<style>
  .nbtnode {
    display: block;
  }
  .nrow {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    background: none;
    border: none;
    font: inherit;
    color: var(--text);
    text-align: left;
    padding: 3px 12px 3px 8px;
    white-space: nowrap;
    min-width: max-content;
  }
  .nrow:not(.leaf) {
    cursor: pointer;
  }
  .nrow:not(.leaf):hover,
  .nrow.leaf:hover {
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .tw {
    width: 12px;
    flex: none;
    color: var(--text-dim);
    font-size: 10px;
    text-align: center;
  }
  .nkey {
    font-family: var(--font-mono);
    font-weight: 600;
    font-size: 12.5px;
    color: var(--text);
    flex: none;
  }
  .nval,
  .ncount {
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ncount {
    color: var(--text-dim);
  }
  .nhint {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--brand-primary);
    flex: none;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 320px;
  }
  .ntag {
    margin-left: auto;
    flex: none;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--tc);
    background: color-mix(in srgb, var(--tc) 15%, transparent);
    border-radius: 5px;
    padding: 1px 7px;
  }
  mark {
    background: color-mix(in srgb, #f5c518 55%, transparent);
    color: inherit;
    border-radius: 2px;
  }
</style>
