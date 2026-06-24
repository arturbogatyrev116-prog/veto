<script>
  import { invoke } from '@tauri-apps/api/core'
  import { conversations, activeConv, user, peerNames, wsStatus, onlinePeers, setPeerName, groups, showSearch, unreadCounts, mutedConvs } from '../stores.js'
  import Avatar from './Avatar.svelte'
  import DeviceManager from './DeviceManager.svelte'

  let newPeerId = ''
  let searchQuery = ''
  let error = ''
  let loading = false
  let searchEl

  let loggingOut = false
  async function logout() {
    if (!confirm('Sign out and delete local data?')) return
    loggingOut = true
    try {
      await invoke('clear_identity')
      user.set(null)
      conversations.set({})
      groups.set({})
      activeConv.set(null)
    } catch (e) {
      alert('Logout failed: ' + e)
    } finally {
      loggingOut = false
    }
  }

  // Theme
  let theme = localStorage.getItem('theme') ?? 'dark'
  $: {
    document.documentElement.setAttribute('data-theme', theme === 'light' ? 'light' : '')
    localStorage.setItem('theme', theme)
  }
  function toggleTheme() { theme = theme === 'dark' ? 'light' : 'dark' }

  // Sound (Web Audio API — no file needed)
  function playNotify() {
    try {
      const ctx = new AudioContext()
      const osc = ctx.createOscillator()
      const gain = ctx.createGain()
      osc.connect(gain); gain.connect(ctx.destination)
      osc.frequency.value = 880
      gain.gain.setValueAtTime(0.15, ctx.currentTime)
      gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.25)
      osc.start(); osc.stop(ctx.currentTime + 0.25)
    } catch {}
  }

  export { playNotify }

  // Filtered conversations
  $: filtered = Object.keys($conversations).filter(pid => {
    if (!searchQuery) return true
    const name = ($peerNames[pid] ?? pid).toLowerCase()
    return name.includes(searchQuery.toLowerCase())
  })

  async function startChat() {
    const peerId = newPeerId.trim()
    if (!peerId) return
    loading = true
    error = ''
    try {
      const { user_id, username } = await invoke('prepare_session', { peerId })
      setPeerName(user_id, username)
      conversations.update(c => ({ ...c, [user_id]: c[user_id] ?? [] }))
      activeConv.set(user_id)
      newPeerId = ''
    } catch (e) {
      error = String(e)
    } finally {
      loading = false
    }
  }

  // Expose focus for Ctrl+K
  export function focusSearch() { searchEl?.focus() }

  // ── New group ────────────────────────────────────────────────────────────────

  let showNewGroup = false
  let newGroupName = ''
  let newGroupMembers = ''
  let groupError = ''
  let groupLoading = false

  async function createGroup() {
    const name = newGroupName.trim()
    if (!name) return
    groupError = ''
    groupLoading = true
    try {
      // Resolve each comma-separated username to a UUID via prepare_session
      const usernames = newGroupMembers.split(',').map(s => s.trim()).filter(Boolean)
      const memberIds = []
      for (const uname of usernames) {
        const { user_id } = await invoke('prepare_session', { peerId: uname })
        memberIds.push(user_id)
      }
      const gi = await invoke('create_group', { name, memberIds })
      groups.update(g => ({ ...g, [gi.group_id]: gi }))
      conversations.update(c => ({ ...c, [gi.group_id]: c[gi.group_id] ?? [] }))
      activeConv.set(gi.group_id)
      newGroupName = ''
      newGroupMembers = ''
      showNewGroup = false
    } catch (e) {
      groupError = String(e)
    } finally {
      groupLoading = false
    }
  }

  // ── Backup / restore ─────────────────────────────────────────────────────────

  let backupPassword = ''
  let restorePassword = ''
  let backupStatus = ''
  let showBackup = false
  let showRestore = false
  let showDeviceManager = false

  // Refresh mute status for all visible conversations
  $: {
    const peers = [...Object.keys($conversations), ...Object.values($groups).map(g => g.group_id)]
    for (const pid of peers) {
      invoke('get_mute', { peerId: pid }).then(s => {
        mutedConvs.update(m => ({ ...m, [pid]: s.is_muted }))
      }).catch(() => {})
    }
  }

  async function doExport() {
    if (!backupPassword) return
    backupStatus = ''
    try {
      const bytes = await invoke('export_identity', { password: backupPassword })
      const blob = new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' })
      const a = document.createElement('a')
      a.href = URL.createObjectURL(blob)
      a.download = 'messenger_identity.mbak'
      a.click()
      URL.revokeObjectURL(a.href)
      backupPassword = ''
      showBackup = false
    } catch (e) {
      backupStatus = String(e)
    }
  }

  async function doImport(file) {
    if (!file || !restorePassword) return
    backupStatus = ''
    try {
      const buf = await file.arrayBuffer()
      const data = Array.from(new Uint8Array(buf))
      const info = await invoke('import_identity', { password: restorePassword, data })
      restorePassword = ''
      showRestore = false
      // Reload: new identity is set, tell the parent to re-init
      window.location.reload()
    } catch (e) {
      backupStatus = String(e)
    }
  }
</script>

<aside>
  <div class="me">
    <div class="me-info">
      <Avatar name={$user?.username ?? ''} uid={$user?.user_id ?? ''} size={28} />
      <span class="username">@{$user?.username}</span>
    </div>
    <div class="me-actions">
      <span
        class="ws-dot"
        class:connected={$wsStatus === 'connected'}
        class:lost={$wsStatus === 'lost'}
        class:reconnecting={$wsStatus === 'reconnecting'}
        title={$wsStatus}
      >
        {$wsStatus === 'connected' ? '●' : $wsStatus === 'lost' ? '✕' : '↻'}
      </span>
      <button class="icon-btn" on:click={() => showSearch.set(true)} title="Search messages (Ctrl+F)">🔍</button>
      <button class="icon-btn" on:click={toggleTheme} title="Toggle theme">
        {theme === 'dark' ? '☀' : '☾'}
      </button>
      <button class="icon-btn logout-btn" on:click={logout} disabled={loggingOut} title="Sign out">⏻</button>
    </div>
  </div>

  <div class="search-wrap">
    <input
      type="text"
      bind:value={searchQuery}
      bind:this={searchEl}
      placeholder="Search (Ctrl+K)"
      class="search"
      on:keydown={e => e.key === 'Escape' && (searchQuery = '')}
    />
  </div>

  <div class="new-chat">
    <form on:submit|preventDefault={startChat}>
      <input
        type="text"
        bind:value={newPeerId}
        placeholder="Username or ID"
        disabled={loading}
      />
      <button type="submit" disabled={loading || !newPeerId.trim()}>+</button>
    </form>
    {#if error}
      <p class="err">{error}</p>
    {/if}
  </div>

  <ul>
    {#each filtered as peerId}
      {@const name = $peerNames[peerId] ?? peerId.slice(0, 8) + '…'}
      {@const unread = $unreadCounts[peerId] ?? 0}
      <li class:active={$activeConv === peerId}>
        <button class="conv-btn" on:click={() => activeConv.set(peerId)}>
          <div class="avatar-wrap">
            <Avatar {name} uid={peerId} size={32} />
            {#if $onlinePeers.has(peerId)}
              <span class="online-dot" title="Online"></span>
            {/if}
          </div>
          <span class="peer-name" class:bold={unread > 0}>{name}</span>
          {#if $mutedConvs[peerId]}
            <span class="muted-icon" title="Muted">🔕</span>
          {:else if unread > 0}
            <span class="badge">{unread > 99 ? '99+' : unread}</span>
          {/if}
        </button>
      </li>
    {/each}
  </ul>

  {#if Object.keys($groups).length > 0 || showNewGroup}
    <div class="section-header">
      <span class="section-label">Groups</span>
      <button class="icon-btn" title="New group" on:click={() => showNewGroup = !showNewGroup}>#</button>
    </div>
    <ul>
      {#each Object.values($groups) as g}
        {@const unread = $unreadCounts[g.group_id] ?? 0}
        <li class:active={$activeConv === g.group_id}>
          <button class="conv-btn" on:click={() => { conversations.update(c => ({ ...c, [g.group_id]: c[g.group_id] ?? [] })); activeConv.set(g.group_id) }}>
            <span class="group-icon">#</span>
            <span class="peer-name" class:bold={unread > 0}>{g.name}</span>
            {#if $mutedConvs[g.group_id]}
              <span class="muted-icon" title="Muted">🔕</span>
            {:else if unread > 0}
              <span class="badge">{unread > 99 ? '99+' : unread}</span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <div class="section-header">
      <span class="section-label">Groups</span>
      <button class="icon-btn" title="New group" on:click={() => showNewGroup = true}>#</button>
    </div>
  {/if}

  {#if showNewGroup}
    <form class="group-form" on:submit|preventDefault={createGroup}>
      <input type="text" bind:value={newGroupName} placeholder="Group name" disabled={groupLoading} />
      <input type="text" bind:value={newGroupMembers} placeholder="Members (user1, user2)" disabled={groupLoading} />
      <div class="group-form-btns">
        <button type="submit" disabled={groupLoading || !newGroupName.trim()}>Create</button>
        <button type="button" on:click={() => { showNewGroup = false; groupError = '' }}>✕</button>
      </div>
      {#if groupError}<p class="err">{groupError}</p>{/if}
    </form>
  {/if}

  <div class="backup-row">
    <button class="icon-btn" title="Manage devices" on:click={() => showDeviceManager = true}>🖥</button>
    <button class="icon-btn" title="Backup identity" on:click={() => { showBackup = !showBackup; showRestore = false; backupStatus = '' }}>⬇</button>
    <button class="icon-btn" title="Restore identity" on:click={() => { showRestore = !showRestore; showBackup = false; backupStatus = '' }}>⬆</button>
  </div>

  {#if showBackup}
    <form class="backup-form" on:submit|preventDefault={doExport}>
      <input type="password" bind:value={backupPassword} placeholder="Backup password" autocomplete="new-password" />
      <button type="submit" disabled={!backupPassword}>Save</button>
    </form>
  {/if}

  {#if showRestore}
    <form class="backup-form" on:submit|preventDefault={() => {}}>
      <input type="password" bind:value={restorePassword} placeholder="Backup password" autocomplete="current-password" />
      <label class="file-btn">
        Load file
        <input type="file" accept=".mbak" style="display:none" on:change={e => doImport(e.target.files[0])} />
      </label>
    </form>
  {/if}

  {#if backupStatus}
    <p class="backup-err">{backupStatus}</p>
  {/if}
</aside>

{#if showDeviceManager}
  <DeviceManager on:close={() => showDeviceManager = false} />
{/if}

<style>
  aside {
    width: 220px;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    flex-shrink: 0;
    transition: width 0.2s;
  }

  .me {
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .me-info { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .username { font-weight: 600; font-size: 13px; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .me-actions { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
  .ws-dot { font-size: 11px; color: var(--text-dim); }
  .ws-dot.connected    { color: var(--success); }
  .ws-dot.lost         { color: var(--danger); }
  .ws-dot.reconnecting { color: var(--accent); animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  .icon-btn {
    background: none;
    color: var(--text-muted);
    padding: 2px 4px;
    font-size: 14px;
    line-height: 1;
  }
  .icon-btn:hover { background: var(--bg-hover); color: var(--text); }
  .logout-btn:hover { background: #3f1212; color: #f87171; }

  .search-wrap { padding: 8px 10px 4px; }
  .search { width: 100%; font-size: 12px; padding: 6px 10px; }

  .new-chat { padding: 6px 10px 8px; border-bottom: 1px solid var(--border); }
  .new-chat form { display: flex; gap: 6px; }
  .new-chat input { flex: 1; min-width: 0; }
  .new-chat button { padding: 8px 10px; }
  .err { color: var(--danger); font-size: 12px; margin-top: 4px; }

  ul { list-style: none; overflow-y: auto; flex: 1; }
  li { border-bottom: 1px solid var(--border-sub); transition: background 0.12s; }
  li:hover  { background: var(--bg-hover); }
  li.active { background: var(--bg-active); }
  .conv-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 12px;
    background: none;
    color: var(--text);
    text-align: left;
    border-radius: 0;
  }
  .avatar-wrap { position: relative; flex-shrink: 0; }
  .online-dot {
    position: absolute;
    bottom: 0; right: 0;
    width: 9px; height: 9px;
    border-radius: 50%;
    background: var(--success);
    border: 2px solid var(--bg-panel);
  }
  .peer-name { flex: 1; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .peer-name.bold { font-weight: 700; color: var(--text); }
  .badge {
    font-size: 10px;
    font-weight: 700;
    color: #fff;
    background: var(--accent);
    border-radius: 999px;
    padding: 1px 5px;
    min-width: 16px;
    text-align: center;
    flex-shrink: 0;
    line-height: 1.5;
  }
  .muted-icon {
    font-size: 12px;
    flex-shrink: 0;
    opacity: 0.6;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px 2px;
    border-top: 1px solid var(--border);
  }
  .section-label { font-size: 11px; font-weight: 600; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.05em; }
  .group-icon { font-size: 14px; color: var(--text-dim); width: 32px; height: 32px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
  .group-form {
    padding: 6px 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    border-bottom: 1px solid var(--border);
  }
  .group-form input { font-size: 12px; padding: 5px 8px; }
  .group-form-btns { display: flex; gap: 5px; }
  .group-form-btns button { flex: 1; font-size: 12px; padding: 5px; }

  .backup-row {
    display: flex;
    justify-content: center;
    gap: 8px;
    padding: 6px 10px;
    border-top: 1px solid var(--border);
  }
  .backup-form {
    display: flex;
    gap: 6px;
    padding: 4px 10px 8px;
  }
  .backup-form input[type="password"] { flex: 1; min-width: 0; font-size: 12px; }
  .backup-form button, .file-btn {
    padding: 6px 8px;
    font-size: 11px;
    cursor: pointer;
    background: var(--bg-hover);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    white-space: nowrap;
  }
  .backup-err { color: var(--danger); font-size: 11px; padding: 0 10px 6px; }

  /* Narrow window — collapse sidebar to icons */
  @media (max-width: 500px) {
    aside { width: 56px; }
    .username, .peer-name, .count, .search-wrap, .new-chat, .ws-dot { display: none; }
    .me { justify-content: center; }
    li { justify-content: center; padding: 10px 0; }
  }
</style>
