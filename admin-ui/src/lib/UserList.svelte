<script>
  export let users = [];
  export let onBlock;
  export let onUnblock;
  export let onDelete;

  let blockReason = {};
  let filter = '';

  $: filtered = users.filter(u =>
    u.username.toLowerCase().includes(filter.toLowerCase())
  );

  function fmt(ts) {
    if (!ts) return '—';
    const d = new Date(ts);
    return d.toLocaleString(undefined, { dateStyle: 'short', timeStyle: 'short' });
  }
</script>

<div class="toolbar">
  <input
    class="search"
    type="search"
    bind:value={filter}
    placeholder="Search username…"
  />
  <span class="count">{filtered.length} / {users.length} users</span>
</div>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th>Username</th>
        <th>Sessions</th>
        <th>Last seen</th>
        <th>Created</th>
        <th>Status</th>
        <th>Actions</th>
      </tr>
    </thead>
    <tbody>
      {#each filtered as u (u.user_id)}
        <tr class:blocked={u.blocked}>
          <td class="mono">{u.username}</td>
          <td class="center">{u.session_count}</td>
          <td>{fmt(u.last_seen)}</td>
          <td>{fmt(u.created_at)}</td>
          <td>
            {#if u.blocked}
              <span class="badge blocked" title={u.blocked_reason || ''}>Blocked</span>
            {:else}
              <span class="badge active">Active</span>
            {/if}
          </td>
          <td class="actions-cell">
            {#if u.blocked}
              <button class="btn unblock" on:click={() => onUnblock(u.user_id)}>Unblock</button>
            {:else}
              <input
                class="reason-input"
                type="text"
                bind:value={blockReason[u.user_id]}
                placeholder="Reason (optional)"
              />
              <button class="btn block" on:click={() => {
                onBlock(u.user_id, blockReason[u.user_id] || '');
                blockReason[u.user_id] = '';
              }}>Block</button>
            {/if}
            <button class="btn delete" on:click={() => onDelete(u.user_id)}>Delete</button>
          </td>
        </tr>
      {/each}

      {#if filtered.length === 0}
        <tr>
          <td colspan="6" class="empty">No users found.</td>
        </tr>
      {/if}
    </tbody>
  </table>
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }

  .search {
    flex: 1;
    max-width: 300px;
    padding: 8px 12px;
    border: 1px solid #ddd;
    border-radius: 6px;
    font-size: 14px;
  }

  .count {
    color: #6c757d;
    font-size: 13px;
  }

  .table-wrap {
    background: white;
    border-radius: 8px;
    box-shadow: 0 1px 4px rgba(0,0,0,.1);
    overflow-x: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 14px;
  }

  th, td {
    padding: 11px 14px;
    text-align: left;
    border-bottom: 1px solid #f0f0f0;
    white-space: nowrap;
  }

  th {
    background: #f8f9fa;
    font-weight: 600;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: .04em;
    color: #6c757d;
  }

  tr:last-child td { border-bottom: none; }

  tr.blocked { background: #fff5f5; }

  .mono { font-family: monospace; }
  .center { text-align: center; }

  .badge {
    display: inline-block;
    padding: 3px 8px;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 600;
  }
  .badge.active  { background: #d1fae5; color: #065f46; }
  .badge.blocked { background: #fee2e2; color: #991b1b; }

  .actions-cell {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .reason-input {
    padding: 5px 8px;
    border: 1px solid #ddd;
    border-radius: 4px;
    font-size: 13px;
    width: 130px;
  }

  .btn {
    padding: 5px 12px;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
    transition: opacity .15s;
  }
  .btn:hover { opacity: .85; }

  .btn.block   { background: #f59e0b; color: #fff; }
  .btn.unblock { background: #10b981; color: #fff; }
  .btn.delete  { background: #ef4444; color: #fff; }

  .empty {
    text-align: center;
    color: #9ca3af;
    padding: 32px;
  }
</style>
