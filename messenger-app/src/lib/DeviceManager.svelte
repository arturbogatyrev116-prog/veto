<script>
  import { onMount, createEventDispatcher } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'

  const dispatch = createEventDispatcher()

  let sessions = []
  let myDeviceId = ''
  let loading = true
  let revoking = null
  let error = ''

  onMount(async () => {
    try {
      [sessions, myDeviceId] = await Promise.all([
        invoke('list_sessions'),
        invoke('get_device_id'),
      ])
    } catch (e) {
      error = String(e)
    } finally {
      loading = false
    }
  })

  async function revoke(sessionId) {
    revoking = sessionId
    error = ''
    try {
      await invoke('revoke_session', { sessionId })
      sessions = sessions.filter(s => s.session_id !== sessionId)
    } catch (e) {
      error = String(e)
    } finally {
      revoking = null
    }
  }

  function formatDate(iso) {
    const d = new Date(iso)
    return d.toLocaleString()
  }

  function isCurrentDevice(s) {
    return s.device_id === myDeviceId
  }
</script>

<div class="dm-overlay" on:click|self={() => dispatch('close')}>
  <div class="dm-panel">
    <div class="dm-header">
      <span class="dm-title">Devices</span>
      <button class="dm-close" on:click={() => dispatch('close')}>✕</button>
    </div>

    {#if loading}
      <div class="dm-empty">Loading…</div>
    {:else if error}
      <div class="dm-error">{error}</div>
    {:else if sessions.length === 0}
      <div class="dm-empty">No active sessions.</div>
    {:else}
      <ul class="dm-list">
        {#each sessions as s (s.session_id)}
          <li class="dm-item" class:current={isCurrentDevice(s)}>
            <div class="dm-device-icon">{isCurrentDevice(s) ? '💻' : '📱'}</div>
            <div class="dm-device-info">
              <div class="dm-device-name">
                {s.device_name}
                {#if isCurrentDevice(s)}
                  <span class="dm-badge">This device</span>
                {/if}
              </div>
              <div class="dm-device-meta">
                Last seen: {formatDate(s.last_seen)}
              </div>
              <div class="dm-device-meta">
                Registered: {formatDate(s.created_at)}
              </div>
            </div>
            {#if !isCurrentDevice(s)}
              <button
                class="dm-revoke"
                disabled={revoking === s.session_id}
                on:click={() => revoke(s.session_id)}
              >
                {revoking === s.session_id ? '…' : 'Revoke'}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .dm-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }

  .dm-panel {
    background: var(--bg-2, #1e1e2e);
    border: 1px solid var(--border, #313244);
    border-radius: 12px;
    width: 420px;
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 24px 64px rgba(0,0,0,0.5);
  }

  .dm-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border, #313244);
    flex-shrink: 0;
  }

  .dm-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text, #cdd6f4);
  }

  .dm-close {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-dim, #6c7086);
    font-size: 14px;
    padding: 2px 6px;
    border-radius: 4px;
    line-height: 1;
  }
  .dm-close:hover { color: var(--text, #cdd6f4); background: var(--bg-3, #313244); }

  .dm-list {
    list-style: none;
    margin: 0;
    padding: 8px;
    overflow-y: auto;
    flex: 1;
  }

  .dm-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    border-radius: 8px;
    margin-bottom: 4px;
  }
  .dm-item.current { background: var(--bg-3, #313244); }
  .dm-item:not(.current):hover { background: var(--bg-3, #313244); }

  .dm-device-icon { font-size: 24px; flex-shrink: 0; }

  .dm-device-info { flex: 1; min-width: 0; }

  .dm-device-name {
    font-size: 14px;
    font-weight: 500;
    color: var(--text, #cdd6f4);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .dm-badge {
    font-size: 10px;
    font-weight: 600;
    color: var(--accent, #89b4fa);
    background: rgba(137, 180, 250, 0.12);
    border-radius: 4px;
    padding: 1px 6px;
  }

  .dm-device-meta {
    font-size: 11px;
    color: var(--text-dim, #6c7086);
    margin-top: 2px;
  }

  .dm-revoke {
    background: none;
    border: 1px solid #f38ba8;
    color: #f38ba8;
    border-radius: 6px;
    font-size: 12px;
    padding: 4px 10px;
    cursor: pointer;
    flex-shrink: 0;
    transition: background 0.15s;
  }
  .dm-revoke:hover:not(:disabled) { background: rgba(243,139,168,0.12); }
  .dm-revoke:disabled { opacity: 0.5; cursor: default; }

  .dm-empty {
    padding: 32px;
    text-align: center;
    color: var(--text-dim, #6c7086);
    font-size: 13px;
  }

  .dm-error {
    padding: 16px 20px;
    color: #f38ba8;
    font-size: 13px;
  }
</style>
