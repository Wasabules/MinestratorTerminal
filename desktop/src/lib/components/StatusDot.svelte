<script lang="ts">
  let { color, pulse = false, size = 9 }: { color: string; pulse?: boolean; size?: number } =
    $props();
</script>

<span
  class="dot"
  class:pulse
  style="--c: {color}; --s: {size}px"
  aria-hidden="true"
></span>

<style>
  .dot {
    width: var(--s);
    height: var(--s);
    border-radius: 50%;
    background: var(--c);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--c) 22%, transparent);
    display: inline-block;
    flex: none;
  }
  .pulse {
    animation: pulse 1.9s ease-in-out infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      box-shadow: 0 0 0 3px color-mix(in srgb, var(--c) 22%, transparent);
    }
    50% {
      box-shadow: 0 0 0 5px color-mix(in srgb, var(--c) 8%, transparent);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse {
      animation: none;
    }
  }
</style>
