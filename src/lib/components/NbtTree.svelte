<script lang="ts">
  // Inspecteur NBT repliable (lecture seule), style NBTExplorer : chaque nœud porte son TYPE NBT
  // exact (puce colorée), les compounds/listes se déplient. Récursif via auto-import.
  import { untrack } from 'svelte';
  import type { NbtNode } from '$lib/types';
  import Self from './NbtTree.svelte';

  let { node, depth = 0 }: { node: NbtNode; depth?: number } = $props();

  const container = $derived(node.tag === 'Compound' || node.tag === 'List');
  const hasChildren = $derived(container && (node.children?.length ?? 0) > 0);
  // Racine + 1er niveau ouverts ; au-delà, replié. `depth` est constant par instance → on capture
  // volontairement sa valeur initiale (untrack pour lever l'avertissement state_referenced_locally).
  let open = $state(untrack(() => depth < 1));

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
</script>

<div class="nbtnode">
  {#if hasChildren}
    <button class="nrow" style="padding-left: {pad}px" onclick={() => (open = !open)}>
      <span class="tw">{open ? '▾' : '▸'}</span>
      {#if node.name != null}<span class="nkey">{node.name}</span>{/if}
      <span class="ncount">{node.len ?? 0}</span>
      <span class="ntag" style="--tc: {meta.color}">{meta.label}</span>
    </button>
  {:else}
    <div class="nrow leaf" style="padding-left: {pad}px">
      <span class="tw"></span>
      {#if node.name != null}<span class="nkey">{node.name}</span>{/if}
      {#if container}
        <span class="ncount">{node.len ?? 0}</span>
      {:else if node.value != null}
        <span class="nval">{node.tag === 'String' ? `"${node.value}"` : node.value}</span>
      {/if}
      <span class="ntag" style="--tc: {meta.color}">{meta.label}</span>
    </div>
  {/if}
  {#if hasChildren && open}
    {#each node.children ?? [] as child, i (i)}
      <Self node={child} depth={depth + 1} />
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
  .nrow:not(.leaf):hover {
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
</style>
