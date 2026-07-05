<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { api, humanizeError } from '$lib/ipc';
  import { copilotEvents } from '$lib/events';
  import { t } from '$lib/i18n';
  import { renderMarkdown } from '$lib/markdown';
  import { uid } from '$lib/util/id';
  import ActionList from '../ActionList.svelte';
  import Icon from '../Icon.svelte';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import type { ProposedAction } from '$lib/types';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const sessionId = $derived(tab.id); // une conversation par onglet

  interface Msg {
    id: string;
    role: 'user' | 'assistant';
    text: string;
    actions: ProposedAction[];
  }

  let messages = $state<Msg[]>([]);
  let input = $state('');
  let sending = $state(false);
  let autonomous = $state(false);
  let currentStep = $state('');
  let elapsedMs = $state(0);

  // Streaming « machine à écrire » à vitesse LISSÉE : les deltas s'accumulent dans `target` ;
  // `shown` (affiché) le rattrape à une vitesse qui varie en douceur (lerp) → animation continue.
  // Le markdown est rendu EN DIRECT mais depuis une copie THROTTLÉE (`shownMd`, ~90 ms) : re-parser +
  // reconstruire le DOM à 60 fps coûtait cher (O(n²) sur les longs messages) — ~11 fps suffisent à l'œil.
  let target = ''; // texte complet reçu (non réactif)
  let shown = $state(''); // texte révélé à l'écran (machine à écrire)
  let shownMd = $state(''); // copie throttlée de `shown` → source du rendu markdown live
  let lastMd = 0; // dernier rafraîchissement markdown (throttle, non réactif)
  let raf: number | undefined;
  let vel = 0; // vitesse de révélation lissée (caractères/frame)
  let acc = 0; // accumulateur fractionnaire

  let scroller: HTMLDivElement;
  let atBottom = true; // l'utilisateur suit-il le bas ? (sinon on ne le « yank » pas)
  let resizeObs: ResizeObserver | undefined;
  let unlisten: UnlistenFn | undefined;
  let unlistenDelta: UnlistenFn | undefined;
  let timer: ReturnType<typeof setInterval> | undefined;
  let destroyed = false; // onglet fermé pendant l'init async ? → ne pas attacher sur un composant mort

  // Tant qu'aucune phase n'est remontée par l'agent, on affiche un message d'amorçage
  // qui évolue avec le temps écoulé — l'utilisateur voit que ça progresse, pas que c'est figé.
  const startingHint = $derived.by(() => {
    const s = elapsedMs / 1000;
    if (s < 3) return t('assistant.starting');
    if (s < 12) return t('assistant.connecting');
    return t('assistant.working');
  });

  const SUGGESTIONS = $derived([
    t('assistant.sugg1'),
    t('assistant.sugg2'),
    t('assistant.sugg3'),
    t('assistant.sugg4'),
    t('assistant.sugg5'),
  ]);

  onMount(async () => {
    unlisten = await copilotEvents.progress((p) => {
      if (p.id === sessionId && sending) currentStep = p.phase;
    });
    if (destroyed) return unlisten(); // onglet fermé pendant l'await → on détache aussitôt
    unlistenDelta = await copilotEvents.chatDelta((d) => {
      if (d.id === sessionId && sending) target += d.text;
    });
    if (destroyed) return unlistenDelta();
    // Quand le panneau (re)devient visible ou est redimensionné, il repasse de taille 0 → on
    // recolle au bas (corrige le 1er message caché sous l'en-tête à l'entrée dans l'onglet).
    if (scroller) {
      resizeObs = new ResizeObserver(() => {
        if (atBottom) scrollToBottom();
      });
      resizeObs.observe(scroller);
      scrollToBottom();
    }
    // F — pré-chauffe le process agent dès l'ouverture de l'onglet (best-effort, silencieux) : le
    // 1er message tape alors un process déjà démarré (Node + serveur MCP prêts).
    void api.chatWarm(sessionId, autonomous).catch(() => {});
  });
  onDestroy(() => {
    destroyed = true;
    unlisten?.();
    unlistenDelta?.();
    resizeObs?.disconnect();
    if (timer) clearInterval(timer);
    if (raf !== undefined) cancelAnimationFrame(raf);
    // Fermeture de l'onglet → on libère la session côté backend (et donc son process agent persistant).
    api.chatReset(sessionId).catch(() => {});
  });

  // Boucle continue (tant que `sending`) : révèle `shown` vers `target` à vitesse lissée.
  function startPump() {
    if (raf !== undefined) return;
    vel = 0;
    acc = 0;
    const loop = () => {
      if (!sending) {
        raf = undefined;
        return;
      }
      const backlog = target.length - shown.length;
      const targetVel = Math.min(14, backlog / 12); // vitesse cible ∝ retard
      vel += (targetVel - vel) * 0.12; // lissage → aucun saut de vitesse
      acc += vel;
      if (backlog > 0 && acc >= 1) {
        const stepN = Math.min(backlog, Math.floor(acc));
        acc -= stepN;
        shown = target.slice(0, shown.length + stepN);
        // Rafraîchit le markdown au plus toutes les ~90 ms (throttle) — ou dès que `shown` a rattrapé
        // `target` (flush) — au lieu de re-parser + reconstruire le DOM à chaque frame.
        const now = performance.now();
        if (now - lastMd > 90 || shown.length === target.length) {
          lastMd = now;
          shownMd = shown;
        }
        if (atBottom) scrollToBottom();
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
  }

  function resetStream() {
    if (raf !== undefined) {
      cancelAnimationFrame(raf);
      raf = undefined;
    }
    target = '';
    shown = '';
    shownMd = '';
    lastMd = 0;
    vel = 0;
    acc = 0;
  }

  function onThreadScroll() {
    if (!scroller) return;
    atBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 60;
  }

  function scrollToBottom() {
    requestAnimationFrame(() => {
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    });
  }

  // Pendant le streaming, coupe le bloc d'actions brut (nettoyé dans la réponse finale).
  function displayStream(s: string): string {
    const i = s.indexOf('===ACTIONS===');
    return i >= 0 ? s.slice(0, i).trimEnd() : s;
  }

  async function send(text?: string) {
    const msg = (text ?? input).trim();
    if (!msg || sending) return;
    input = '';
    messages.push({ id: uid(), role: 'user', text: msg, actions: [] });
    sending = true;
    currentStep = '';
    resetStream();
    atBottom = true; // on envoie → on suit le bas
    elapsedMs = 0;
    const startedAt = performance.now();
    timer = setInterval(() => (elapsedMs = performance.now() - startedAt), 250);
    scrollToBottom();
    startPump();
    try {
      const reply = await api.chatSend(sessionId, serverId, tab.serverName, msg, autonomous);
      // Filet de sécurité : si le backend renvoyait un texte vide (cas extrême), on retombe sur le
      // texte déjà streamé — le frontend l'a reçu intégralement, insensible à toute troncature backend.
      const text = reply.text.trim() || displayStream(target).trim();
      messages.push({ id: uid(), role: 'assistant', text, actions: reply.actions });
    } catch (e) {
      messages.push({ id: uid(), role: 'assistant', text: '⚠ ' + humanizeError(e), actions: [] });
    } finally {
      sending = false;
      currentStep = '';
      resetStream();
      if (timer) {
        clearInterval(timer);
        timer = undefined;
      }
      scrollToBottom();
    }
  }

  function reset() {
    api.chatReset(sessionId).catch(() => {});
    messages = [];
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }
</script>

<div class="chat">
  <header class="bar">
    <div class="left">
      <span class="ico"><Icon name="message-circle" size={17} /></span>
      <span class="ttl">{t('assistant.title')}</span>
      <span class="srv dim">{tab.serverName}</span>
    </div>
    <div class="right">
      <label class="auto" title={t('assistant.autonomousHint')}>
        <input type="checkbox" bind:checked={autonomous} />
        <span>{t('assistant.autonomous')}</span>
      </label>
      {#if messages.length > 0}
        <button class="reset" onclick={reset}>{t('assistant.reset')}</button>
      {/if}
    </div>
  </header>

  <div class="thread" bind:this={scroller} onscroll={onThreadScroll}>
    {#if messages.length === 0}
      <div class="empty">
        <div class="hi">{t('assistant.intro')}</div>
        <div class="suggs">
          {#each SUGGESTIONS as s (s)}
            <button class="sugg" onclick={() => send(s)}>{s}</button>
          {/each}
        </div>
      </div>
    {/if}

    {#each messages as m (m.id)}
      <div class="msg {m.role}">
        <div class="bubble">
          {#if m.role === 'assistant'}
            <div class="md">{@html renderMarkdown(m.text)}</div>
          {:else}
            <div class="text">{m.text}</div>
          {/if}
          {#if m.actions.length > 0}
            <div class="msg-actions">
              <ActionList actions={m.actions} {serverId} />
            </div>
          {/if}
        </div>
      </div>
    {/each}

    {#if sending}
      <div class="msg assistant">
        <div class="bubble">
          {#if displayStream(shownMd)}
            <!-- Rendu markdown EN DIRECT depuis la copie THROTTLÉE `shownMd` (~90 ms) : le parser tolère
                 le markdown incomplet (bloc de code non refermé qui grandit, inline littéral puis basculé) ;
                 transition vers le message final markdown→markdown, sans reflow ni re-parse à 60 fps. -->
            <div class="md stream">{@html renderMarkdown(displayStream(shownMd))}</div>
          {/if}
          <div class="thinking" class:has-text={shown}>
            <span class="dots"><span></span><span></span><span></span></span>
            <span class="step dim">{currentStep || startingHint}</span>
            {#if elapsedMs > 900}
              <span class="elapsed dim">{Math.floor(elapsedMs / 1000)}s</span>
            {/if}
          </div>
        </div>
      </div>
    {/if}
  </div>

  <form class="composer" onsubmit={(e) => { e.preventDefault(); void send(); }}>
    <textarea
      class="input"
      bind:value={input}
      onkeydown={onKey}
      placeholder={t('assistant.placeholder')}
      rows="1"
      disabled={sending}
    ></textarea>
    <button class="send" type="submit" disabled={sending || input.trim().length === 0}>
      {t('console.send')}
    </button>
  </form>
</div>

<style>
  /* Grille « en-tête / corps scrollable / pied » — le pattern le PLUS fiable (pas de dépendance
     aux quirks de flex min-height ni au positionnement absolu). `minmax(0,1fr)` autorise le
     `.thread` à rétrécir sous son contenu → il scrolle proprement, seul, sans déborder.
     NB : classe `.chat` et NON `.assistant` — un message d'agent est `.msg.assistant`, donc une
     règle `.assistant` fuirait dessus (héritait `height:100%` → message étiré à toute la hauteur). */
  .chat {
    height: 100%;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    min-height: 0;
    overflow: hidden;
  }
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .left {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    color: var(--brand-primary);
  }
  .ttl {
    font-weight: 700;
    font-size: 14px;
  }
  .srv {
    font-size: 12.5px;
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .right {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: none;
  }
  .auto {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    color: var(--text-muted);
    cursor: pointer;
    white-space: nowrap;
  }
  .auto input {
    accent-color: var(--brand-primary);
    cursor: pointer;
  }
  .reset {
    background: none;
    border: 1px solid var(--border);
    border-radius: 7px;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 5px 10px;
  }
  .reset:hover {
    color: var(--text);
  }
  /* Corps scrollable = flex COLUMN. Les messages sont des items `flex:0 0 auto` → hauteur = contenu,
     jamais étirés (la vraie cause de l'étirement — la collision de classe `.assistant` racine vs
     `.msg.assistant` qui forçait `height:100%` — a été supprimée, cf `.chat`). `gap` gère l'espacement
     inter-messages (plus propre que des margins) ; `overflow-y:auto` scrolle sous le contenu. */
  .thread {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    justify-content: flex-start;
    gap: 12px;
    min-height: 0;
    overflow-y: auto;
    padding: 18px 16px;
  }
  .empty {
    margin: 8vh auto 0;
    max-width: 460px;
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .hi {
    font-size: 14.5px;
    color: var(--text-muted);
    line-height: 1.5;
  }
  .suggs {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .sugg {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    font: inherit;
    font-size: 13.5px;
    color: var(--text);
    padding: 11px 14px;
    text-align: left;
  }
  .sugg:hover {
    border-color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 8%, transparent);
  }
  .msg {
    display: flex; /* item flex : ligne contenant la bulle (alignée à droite pour l'utilisateur) */
    flex: 0 0 auto; /* hauteur = contenu, aucun shrink → le thread scrolle au lieu de comprimer */
    align-items: flex-start; /* la bulle garde sa hauteur de contenu, jamais étirée */
    animation: msg-in 0.22s ease-out; /* entrée douce : fondu + léger glissement vers le haut */
  }
  @keyframes msg-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  /* Accessibilité : pas d'animation si l'utilisateur a demandé à réduire les mouvements. */
  @media (prefers-reduced-motion: reduce) {
    .msg {
      animation: none;
    }
  }
  .msg.user {
    justify-content: flex-end;
  }
  .bubble {
    max-width: 80%;
    border-radius: var(--radius-lg);
    padding: 11px 14px;
    font-size: 13.5px;
    line-height: 1.55;
  }
  .msg.user .bubble {
    background: var(--brand-primary);
    color: #fff;
    border-bottom-right-radius: 4px;
  }
  .msg.assistant .bubble {
    background: var(--surface);
    border: 1px solid var(--border);
    border-bottom-left-radius: 4px;
  }
  .text {
    white-space: pre-wrap;
    word-break: break-word;
  }
  /* Rendu markdown (.md) — stylé globalement dans app.css, partagé par le streaming (.md.stream)
     et le message final pour une transition sans reflow. La progression est signalée par les
     points animés (.thinking) sous le contenu, plus besoin d'un curseur de frappe. */
  .thinking {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .thinking.has-text {
    margin-top: 8px;
    opacity: 0.8;
  }
  .dots {
    display: inline-flex;
    align-items: flex-end;
    gap: 4px;
    height: 12px;
  }
  .dots span {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--brand-primary);
    animation: dot-bounce 1.3s ease-in-out infinite;
  }
  .dots span:nth-child(2) {
    animation-delay: 0.16s;
  }
  .dots span:nth-child(3) {
    animation-delay: 0.32s;
  }
  /* Rebond vertical (typing indicator classique) : les points sautent tour à tour. */
  @keyframes dot-bounce {
    0%,
    70%,
    100% {
      transform: translateY(0);
      opacity: 0.5;
    }
    35% {
      transform: translateY(-6px);
      opacity: 1;
    }
  }
  /* Pulsation d'opacité de repli (jamais figé) quand les animations OS sont réduites. */
  @keyframes dot-fade {
    0%,
    100% {
      opacity: 0.35;
    }
    50% {
      opacity: 1;
    }
  }
  .step {
    font-size: 12.5px;
  }
  .elapsed {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    opacity: 0.6;
    margin-left: auto;
    padding-left: 8px;
  }
  @media (prefers-reduced-motion: reduce) {
    /* On garde une pulsation douce (sans déplacement) plutôt que de tout figer. */
    .dots span {
      animation: dot-fade 1.3s ease-in-out infinite;
    }
  }
  .msg-actions {
    margin-top: 12px;
  }
  .composer {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border);
    flex: none;
  }
  .input {
    flex: 1;
    resize: none;
    max-height: 160px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
    padding: 10px 12px;
    line-height: 1.4;
  }
  .input:focus {
    outline: none;
    border-color: var(--brand-primary);
  }
  .send {
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: 10px;
    padding: 10px 18px;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
    flex: none;
  }
  .send:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
