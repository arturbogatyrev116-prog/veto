<script>
  import { invoke } from '@tauri-apps/api/core'
  import { createEventDispatcher, onMount } from 'svelte'

  export let peerId
  export let peerName

  const dispatch = createEventDispatcher()

  let number = ''
  let error = ''
  let loading = true

  onMount(async () => {
    try {
      number = await invoke('get_safety_number', { peerId })
    } catch (e) {
      error = String(e)
    } finally {
      loading = false
    }
  })

  // Split into 6 groups for large display
  $: groups = number ? number.split(' ') : []

  function close() { dispatch('close') }

  function onKey(e) { if (e.key === 'Escape') close() }
</script>

<svelte:window on:keydown={onKey} />

<!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
<div class="overlay" role="dialog" aria-modal="true" on:click|self={close}>
  <div class="modal">
    <div class="modal-header">
      <span class="title">Safety Number</span>
      <button class="close-btn" on:click={close} aria-label="Close">✕</button>
    </div>

    <p class="description">
      Verify your conversation with <strong>{peerName}</strong> by comparing
      these numbers in person or through a trusted channel.
      If they match — your connection is secure.
    </p>

    {#if loading}
      <div class="loading">Computing…</div>
    {:else if error}
      <p class="err">{error}</p>
    {:else}
      <div class="number-grid">
        {#each groups as group}
          <span class="group">{group}</span>
        {/each}
      </div>
      <p class="hint">
        🔒 Both parties should see identical numbers.
      </p>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 500;
    backdrop-filter: blur(2px);
  }

  .modal {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 28px 32px;
    width: 380px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .title {
    font-size: 17px;
    font-weight: 700;
    color: var(--text);
  }

  .close-btn {
    background: none;
    color: var(--text-muted);
    padding: 4px 8px;
    font-size: 14px;
    border-radius: var(--radius);
  }
  .close-btn:hover { background: var(--bg-hover); color: var(--text); }

  .description {
    font-size: 13px;
    color: var(--text-muted);
    line-height: 1.55;
    margin: 0;
  }

  .number-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
    margin: 4px 0;
  }

  .group {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 0;
    text-align: center;
    font-family: monospace;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: var(--accent);
  }

  .hint {
    font-size: 12px;
    color: var(--text-dim);
    text-align: center;
    margin: 0;
  }

  .loading {
    text-align: center;
    color: var(--text-muted);
    padding: 16px 0;
  }

  .err {
    color: var(--danger);
    font-size: 13px;
    margin: 0;
  }
</style>
