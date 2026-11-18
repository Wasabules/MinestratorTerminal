<script lang="ts">
  /** Fenêtre modale des dialogues MyBox (création, redimensionnement, suppression, confirmation). */
  import type { Snippet } from 'svelte';
  import { t } from '#lib/i18n/index.ts';

  let {
    title,
    onclose,
    children,
    width = 520,
  }: { title: string; onclose: () => void; children: Snippet; width?: number } = $props();

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<button class="mb-backdrop" aria-label={t('common.cancel')} onclick={onclose}></button>
<div
  class="mb-modal card"
  role="dialog"
  aria-modal="true"
  aria-label={title}
  style="width: min({width}px, calc(100vw - 40px))"
>
  <h2>{title}</h2>
  {@render children()}
</div>

<style>
  .mb-backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
    border: none;
    cursor: default;
    background: color-mix(in srgb, #000 45%, transparent);
    backdrop-filter: blur(2px);
  }
  .mb-modal {
    position: fixed;
    z-index: 91;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    max-height: calc(100vh - 60px);
    overflow-y: auto;
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
  }
</style>
