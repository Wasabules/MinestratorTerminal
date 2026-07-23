<script lang="ts">
  import { t } from '$lib/i18n';
  import { viewMeta, type ServerTab } from '$lib/tabs/tabs.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const meta = $derived(viewMeta(tab.view));
</script>

<div class="pad">
  <div class="placeholder">
    <span class="g"><Icon name={meta.icon} size={34} stroke={1.5} /></span>
    <h2>{t(`view.${tab.view}`)} — {tab.serverName}</h2>
    <p>{t('common.comingSoon')}</p>
  </div>
</div>

<style>
  .pad {
    padding: 28px 24px;
    display: flex;
    justify-content: center;
  }
  .placeholder {
    max-width: 480px;
    text-align: center;
    border: 1px dashed var(--border);
    border-radius: var(--radius-lg);
    padding: 40px 28px;
    color: var(--text-muted);
  }
  .g {
    display: inline-flex;
    color: var(--brand-primary);
  }
  h2 {
    margin: 12px 0 8px;
    font-size: 18px;
    color: var(--text);
  }
  p {
    margin: 0;
    font-size: 13.5px;
  }
</style>
