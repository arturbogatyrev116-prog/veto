<script>
  import { invoke } from '@tauri-apps/api/core'
  import { activeConv, peerNames, groups, showSearch, conversations } from '../stores.js'

  let query = ''
  let results = []
  let searching = false
  let inputEl
  let debounce

  $: if ($showSearch && inputEl) setTimeout(() => inputEl?.focus(), 50)

  function onQueryInput() {
    clearTimeout(debounce)
    if (query.trim().length < 2) { results = []; return }
    debounce = setTimeout(doSearch, 250)
  }

  async function doSearch() {
    searching = true
    try {
      results = await invoke('search_messages', { query: query.trim(), limit: 30 })
    } catch { results = [] }
    searching = false
  }

  function navigate(hit) {
    // Ensure the conversation slot exists so clicking it shows the chat.
    conversations.update(c => ({ ...c, [hit.peer_id]: c[hit.peer_id] ?? [] }))
    activeConv.set(hit.peer_id)
    showSearch.set(false)
  }

  function close() { showSearch.set(false) }

  function onKeydown(e) {
    if (e.key === 'Escape') close()
  }

  // Format «matched» → highlighted span after HTML-escaping.
  function formatSnippet(raw) {
    const escaped = raw
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
    return escaped
      .replace(/&lt;&lt;/g, '<mark>')
      .replace(/&gt;&gt;/g, '</mark>')
  }

  function convName(hit) {
    if (hit.group_id && $groups[hit.group_id]) return '#' + $groups[hit.group_id].name
    return $peerNames[hit.peer_id] ?? hit.peer_id.slice(0, 8) + '…'
  }

  function formatTs(ts) {
    const d = new Date(ts)
    const now = new Date()
    const diff = now - d
    if (diff < 86400000) return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    if (diff < 604800000) return d.toLocaleDateString([], { weekday: 'short' })
    return d.toLocaleDateString([], { month: 'short', day: 'numeric' })
  }
</script>

<svelte:window on:keydown={onKeydown} />

{#if $showSearch}
<div class="overlay" on:click|self={close} role="dialog" aria-modal="true">
  <div class="modal">
    <div class="header">
      <input
        bind:this={inputEl}
        bind:value={query}
        on:input={onQueryInput}
        placeholder="Search messages…"
        class="search-input"
        spellcheck="false"
      />
      <button class="close-btn" on:click={close} title="Close (Esc)">✕</button>
    </div>

    <div class="results">
      {#if searching}
        <div class="status">Searching…</div>
      {:else if query.trim().length >= 2 && results.length === 0}
        <div class="status">No results</div>
      {:else}
        {#each results as hit (hit.db_id)}
          <button class="hit" on:click={() => navigate(hit)}>
            <div class="hit-meta">
              <span class="hit-conv">{convName(hit)}</span>
              <span class="hit-dir">{hit.direction === 'sent' ? 'You' : ''}</span>
              <span class="hit-ts">{formatTs(hit.ts)}</span>
            </div>
            <div class="hit-snippet">{@html formatSnippet(hit.snippet)}</div>
          </button>
        {/each}
        {#if results.length === 30}
          <div class="status muted">Showing first 30 results — refine your query</div>
        {/if}
      {/if}
    </div>
  </div>
</div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.55);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 80px;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    width: min(560px, 92vw);
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 60px rgba(0,0,0,0.4);
    overflow: hidden;
  }

  .header {
    display: flex;
    align-items: center;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    gap: 8px;
  }

  .search-input {
    flex: 1;
    font-size: 15px;
    padding: 6px 10px;
    border: none;
    background: var(--bg-input);
    color: var(--text);
    border-radius: 6px;
    outline: none;
  }
  .search-input::placeholder { color: var(--text-dim); }

  .close-btn {
    background: none;
    color: var(--text-dim);
    font-size: 16px;
    padding: 4px 6px;
    flex-shrink: 0;
  }
  .close-btn:hover { color: var(--text); background: var(--bg-hover); }

  .results {
    overflow-y: auto;
    flex: 1;
  }

  .status {
    padding: 20px;
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
  }
  .status.muted { padding: 8px 16px; font-size: 11px; }

  .hit {
    display: block;
    width: 100%;
    text-align: left;
    padding: 10px 14px;
    background: none;
    color: var(--text);
    border-bottom: 1px solid var(--border-sub);
    border-radius: 0;
    cursor: pointer;
  }
  .hit:hover { background: var(--bg-hover); }

  .hit-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 3px;
  }
  .hit-conv { font-weight: 600; font-size: 13px; color: var(--accent); }
  .hit-dir  { font-size: 11px; color: var(--text-dim); }
  .hit-ts   { font-size: 11px; color: var(--text-dim); margin-left: auto; }

  .hit-snippet {
    font-size: 13px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.4;
  }

  .hit-snippet :global(mark) {
    background: var(--accent);
    color: #fff;
    border-radius: 2px;
    padding: 0 1px;
    font-style: normal;
  }
</style>
