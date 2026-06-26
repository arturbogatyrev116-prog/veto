<script>
  import { createEventDispatcher, onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { peerNames, groups } from '../stores.js'

  const dispatch = createEventDispatcher()

  let stats = null
  let loading = true
  let error = ''

  onMount(async () => {
    try {
      stats = await invoke('get_chat_stats')
    } catch (e) {
      error = String(e)
    } finally {
      loading = false
    }
  })

  function fmt(bytes) {
    if (bytes == null || bytes === 0) return '0 B'
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
  }

  function fmtDate(ts) {
    if (!ts) return '—'
    return new Date(ts).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
  }

  function peerLabel(id) {
    if (id === '__saved__') return '🔖 Saved Messages'
    if ($groups[id]) return `# ${$groups[id].name}`
    return $peerNames[id] ?? id.slice(0, 10) + '…'
  }
</script>

<svelte:window on:keydown={e => e.key === 'Escape' && dispatch('close')} />

<!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
<div class="overlay" role="dialog" aria-modal="true" on:click|self={() => dispatch('close')}>
  <div class="modal">
    <div class="modal-header">
      <span class="modal-title">📊 Chat Statistics</span>
      <button class="close-btn" on:click={() => dispatch('close')}>×</button>
    </div>

    {#if loading}
      <div class="loading">Loading…</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else if stats}
      <div class="summary">
        <div class="stat-card">
          <div class="stat-value">{stats.total_msgs.toLocaleString()}</div>
          <div class="stat-label">Total messages</div>
        </div>
        <div class="stat-card">
          <div class="stat-value">{stats.peers.length}</div>
          <div class="stat-label">Conversations</div>
        </div>
        <div class="stat-card">
          <div class="stat-value">{fmt(stats.total_file_bytes)}</div>
          <div class="stat-label">Files transferred</div>
        </div>
        <div class="stat-card">
          <div class="stat-value">{fmt(stats.db_size_bytes)}</div>
          <div class="stat-label">DB size</div>
        </div>
      </div>

      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Conversation</th>
              <th class="num">Messages</th>
              <th class="num">Sent</th>
              <th class="num">Received</th>
              <th class="num">Files</th>
              <th class="num">File size</th>
              <th class="num">Since</th>
            </tr>
          </thead>
          <tbody>
            {#each stats.peers as p}
              <tr>
                <td class="peer-cell">{peerLabel(p.peer_id)}</td>
                <td class="num">{p.msg_count.toLocaleString()}</td>
                <td class="num">{p.sent_count.toLocaleString()}</td>
                <td class="num">{p.recv_count.toLocaleString()}</td>
                <td class="num">{p.file_count > 0 ? p.file_count.toLocaleString() : '—'}</td>
                <td class="num">{p.file_bytes > 0 ? fmt(p.file_bytes) : '—'}</td>
                <td class="num date">{fmtDate(p.oldest_ts)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed; inset: 0; z-index: 200;
    background: rgba(0,0,0,0.55);
    display: flex; align-items: center; justify-content: center;
  }
  .modal {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    width: min(820px, 95vw);
    max-height: 85vh;
    display: flex; flex-direction: column;
    box-shadow: 0 16px 48px rgba(0,0,0,0.5);
  }
  .modal-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .modal-title { font-size: 15px; font-weight: 600; color: var(--text); }
  .close-btn {
    background: none; border: none; color: var(--text-muted); font-size: 20px;
    cursor: pointer; padding: 0 4px; line-height: 1;
  }
  .close-btn:hover { color: var(--text); }

  .loading, .error {
    padding: 40px; text-align: center; color: var(--text-muted); font-size: 14px;
  }
  .error { color: var(--danger); }

  .summary {
    display: flex; gap: 12px; padding: 16px 20px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0; flex-wrap: wrap;
  }
  .stat-card {
    flex: 1; min-width: 120px;
    background: var(--bg-hover);
    border-radius: 8px; padding: 12px 14px;
    text-align: center;
  }
  .stat-value { font-size: 22px; font-weight: 700; color: var(--accent); }
  .stat-label { font-size: 11px; color: var(--text-muted); margin-top: 2px; }

  .table-wrap { overflow-y: auto; flex: 1; }
  table { width: 100%; border-collapse: collapse; font-size: 12px; }
  thead tr { position: sticky; top: 0; background: var(--bg-panel); z-index: 1; }
  th {
    padding: 8px 12px; text-align: left; font-size: 11px; font-weight: 600;
    color: var(--text-dim); border-bottom: 1px solid var(--border);
    text-transform: uppercase; letter-spacing: 0.04em;
  }
  td { padding: 7px 12px; border-bottom: 1px solid var(--border-sub); color: var(--text); }
  tr:hover td { background: var(--bg-hover); }
  .num { text-align: right; }
  .date { color: var(--text-muted); }
  .peer-cell { max-width: 180px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
