<script>
  import { invoke } from '@tauri-apps/api/core'
  import { conversations, activeConv, user, peerNames, wsStatus, onlinePeers, setPeerName, groups, showSearch, unreadCounts, mutedConvs, channels } from '../stores.js'
  import Avatar from './Avatar.svelte'
  import DeviceManager from './DeviceManager.svelte'
  import ChatStats from './ChatStats.svelte'
  import SettingsModal from './SettingsModal.svelte'

  export let collapsed = false

  let newPeerId = ''
  let searchQuery = ''
  let error = ''
  let loading = false
  let searchEl
  let newChatEl

  let showProfileEdit = false
  let profileDisplayName = ''
  let profileSaving = false
  let profileError = ''

  async function saveProfile() {
    if (!profileDisplayName.trim()) return
    profileSaving = true; profileError = ''
    try {
      await invoke('update_profile', { displayName: profileDisplayName.trim() })
      showProfileEdit = false
    } catch (e) {
      profileError = String(e)
    } finally {
      profileSaving = false }
  }

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

  // ── Appearance ────────────────────────────────────────────────────────────
  let theme  = localStorage.getItem('theme')  ?? 'system'
  let accent = localStorage.getItem('accent') ?? 'blue'
  let showThemeMenu = false

  const ACCENTS = {
    blue:   { label: 'Blue',   hex: '#3b82f6', accentH: '#2563eb', dark: '#1d4ed8', light: '#3b82f6', bgActD: '#1d3a5f', bgActL: '#dbeafe' },
    purple: { label: 'Purple', hex: '#8b5cf6', accentH: '#7c3aed', dark: '#5b21b6', light: '#8b5cf6', bgActD: '#2e1065', bgActL: '#ede9fe' },
    green:  { label: 'Green',  hex: '#22c55e', accentH: '#16a34a', dark: '#15803d', light: '#22c55e', bgActD: '#052e16', bgActL: '#dcfce7' },
    pink:   { label: 'Pink',   hex: '#ec4899', accentH: '#db2777', dark: '#9d174d', light: '#ec4899', bgActD: '#500724', bgActL: '#fce7f3' },
    orange: { label: 'Orange', hex: '#f97316', accentH: '#ea580c', dark: '#9a3412', light: '#f97316', bgActD: '#431407', bgActL: '#ffedd5' },
    teal:   { label: 'Teal',   hex: '#14b8a6', accentH: '#0d9488', dark: '#0f766e', light: '#14b8a6', bgActD: '#042f2e', bgActL: '#ccfbf1' },
  }

  function applyAppearance(t, a) {
    const resolved = t === 'system'
      ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
      : t
    const isLight = resolved === 'light'
    document.documentElement.setAttribute('data-theme', isLight ? 'light' : '')
    localStorage.setItem('theme', t)
    const p = ACCENTS[a] ?? ACCENTS.blue
    const root = document.documentElement
    root.style.setProperty('--accent',      p.hex)
    root.style.setProperty('--accent-h',    p.accentH)
    root.style.setProperty('--bg-msg-out',  isLight ? p.light  : p.dark)
    root.style.setProperty('--bg-active',   isLight ? p.bgActL : p.bgActD)
    localStorage.setItem('accent', a)
  }

  $: applyAppearance(theme, accent)

  if (typeof window !== 'undefined') {
    window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
      if (theme === 'system') applyAppearance('system', accent)
    })
  }

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

  let folder = 'all'   // 'all' | 'dms' | 'groups'

  // Filtered conversations
  $: filtered = Object.keys($conversations).filter(pid => {
    if (pid === '__saved__') return false
    if ($groups[pid]) return false
    if (!searchQuery) return true
    const name = ($peerNames[pid] ?? pid).toLowerCase()
    return name.includes(searchQuery.toLowerCase())
  })

  $: dmUnread    = filtered.reduce((s, pid) => s + ($unreadCounts[pid] ?? 0), 0)
  $: groupUnread = Object.keys($groups).reduce((s, gid) => s + ($unreadCounts[gid] ?? 0), 0)

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

  // Expose focus for Ctrl+K and Ctrl+N
  export function focusSearch() { searchEl?.focus() }
  export function focusNewChat() { newChatEl?.focus() }

  // ── Channels ─────────────────────────────────────────────────────────────────

  let channelExpandedGroups = {}   // { [group_id]: boolean }
  let showNewChannelFor = null     // group_id | null
  let newChannelName = ''
  let newChannelDesc = ''
  let channelError = ''
  let channelLoading = false

  // Load channels whenever groups change
  $: {
    for (const gid of Object.keys($groups)) {
      if (!$channels[gid]) {
        invoke('load_channels', { groupId: gid }).then(list => {
          channels.update(ch => ({ ...ch, [gid]: list }))
        }).catch(() => {})
      }
    }
  }

  function myRoleIn(gid) {
    return $groups[gid]?.members.find(m => m.user_id === $user?.user_id)?.role ?? 'member'
  }

  function canManageChannels(gid) {
    const r = myRoleIn(gid)
    return r === 'owner' || r === 'admin'
  }

  async function createChannel(gid) {
    const name = newChannelName.trim()
    if (!name) return
    channelError = ''; channelLoading = true
    try {
      const ch = await invoke('create_channel', { groupId: gid, name, description: newChannelDesc.trim() || null })
      channels.update(cs => ({ ...cs, [gid]: [...(cs[gid] ?? []), ch] }))
      newChannelName = ''; newChannelDesc = ''
      showNewChannelFor = null
    } catch (e) {
      channelError = String(e)
    } finally {
      channelLoading = false
    }
  }

  function openChannel(gid, cid) {
    conversations.update(c => ({ ...c, [cid]: c[cid] ?? [] }))
    activeConv.set(`${gid}/${cid}`)
  }

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

  // ── DND / Focus mode ─────────────────────────────────────────────────────────

  let dndEnabled = localStorage.getItem('dnd_enabled') === 'true'
  let dndFrom    = localStorage.getItem('dnd_from') ?? '22:00'
  let dndTo      = localStorage.getItem('dnd_to')   ?? '08:00'
  let showDnd    = false

  function saveDnd() {
    localStorage.setItem('dnd_enabled', String(dndEnabled))
    localStorage.setItem('dnd_from', dndFrom)
    localStorage.setItem('dnd_to', dndTo)
  }

  function toggleDnd() { dndEnabled = !dndEnabled; saveDnd() }

  // ── Backup / restore ─────────────────────────────────────────────────────────

  let backupPassword = ''
  let restorePassword = ''
  let backupStatus = ''
  let showBackup = false
  let showRestore = false
  let showDeviceManager = false
  let showChatStats = false
  let showSettings = false

  // ── Screen capture protection ─────────────────────────────────────────────────
  let screenProtected = localStorage.getItem('screen_protected') === 'true'

  async function toggleScreenProtection() {
    screenProtected = !screenProtected
    localStorage.setItem('screen_protected', String(screenProtected))
    try { await invoke('set_screen_capture_protection', { enabled: screenProtected }) } catch {}
  }

  // Apply screen protection reactively (also covers on-load)
  $: invoke('set_screen_capture_protection', { enabled: screenProtected }).catch(() => {})

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

<svelte:window
  on:click={() => showThemeMenu = false}
  on:keydown={e => e.key === 'Escape' && (showThemeMenu = false)}
/>

<aside class:collapsed>
  <div class="me">
    <div class="me-info">
      <button class="avatar-edit-btn" title="Edit profile" on:click={() => { showProfileEdit = !showProfileEdit; profileDisplayName = $user?.username ?? ''; profileError = '' }}>
        <Avatar name={$user?.username ?? ''} uid={$user?.user_id ?? ''} size={28} />
      </button>
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
      <div class="theme-wrap">
        <button class="icon-btn" aria-label="Theme and accent colors" title="Theme & colors" on:click|stopPropagation={() => showThemeMenu = !showThemeMenu}>🎨</button>
        {#if showThemeMenu}
          <div class="theme-menu" role="menu" tabindex="0" on:click|stopPropagation on:keydown|stopPropagation>
            <div class="tm-section">
              <div class="tm-label">Theme</div>
              <div class="tm-pills">
                <button class="tm-pill" class:selected={theme === 'dark'}   on:mousedown|preventDefault on:click={() => theme = 'dark'}>Dark</button>
                <button class="tm-pill" class:selected={theme === 'system'} on:mousedown|preventDefault on:click={() => theme = 'system'}>Auto</button>
                <button class="tm-pill" class:selected={theme === 'light'}  on:mousedown|preventDefault on:click={() => theme = 'light'}>Light</button>
              </div>
            </div>
            <div class="tm-section">
              <div class="tm-label">Accent</div>
              <div class="accent-swatches">
                {#each Object.entries(ACCENTS) as [key, p]}
                  <button
                    class="accent-swatch"
                    class:selected={accent === key}
                    style="background:{p.hex}"
                    aria-label={p.label}
                    title={p.label}
                    on:mousedown|preventDefault
                    on:click={() => accent = key}
                  ></button>
                {/each}
              </div>
            </div>
          </div>
        {/if}
      </div>
      <button class="icon-btn" title="Settings" aria-label="Settings" on:click={() => showSettings = true}>⚙</button>
      <button class="icon-btn logout-btn" on:click={logout} disabled={loggingOut} title="Sign out">⏻</button>
    </div>
  </div>

  {#if showProfileEdit}
    <form class="profile-form" on:submit|preventDefault={saveProfile}>
      <!-- svelte-ignore a11y-autofocus -->
      <input
        type="text"
        bind:value={profileDisplayName}
        placeholder="Display name"
        maxlength="64"
        disabled={profileSaving}
        autofocus
      />
      <button type="submit" disabled={profileSaving || !profileDisplayName.trim()}>
        {profileSaving ? '…' : 'Save'}
      </button>
      <button type="button" on:click={() => showProfileEdit = false}>✕</button>
      {#if profileError}<p class="err">{profileError}</p>{/if}
    </form>
  {/if}

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
        bind:this={newChatEl}
        placeholder="Username or ID (Ctrl+N)"
        disabled={loading}
      />
      <button type="submit" disabled={loading || !newPeerId.trim()}>+</button>
    </form>
    {#if error}
      <p class="err">{error}</p>
    {/if}
  </div>

  <div class="folder-tabs">
    <button class="ftab" class:active={folder === 'all'} on:click={() => folder = 'all'}>
      All
      {#if folder !== 'all' && dmUnread + groupUnread > 0}
        <span class="ftab-badge">{Math.min(dmUnread + groupUnread, 99)}</span>
      {/if}
    </button>
    <button class="ftab" class:active={folder === 'dms'} on:click={() => folder = 'dms'}>
      DMs
      {#if folder !== 'dms' && dmUnread > 0}
        <span class="ftab-badge">{Math.min(dmUnread, 99)}</span>
      {/if}
    </button>
    <button class="ftab" class:active={folder === 'groups'} on:click={() => folder = 'groups'}>
      Groups
      {#if folder !== 'groups' && groupUnread > 0}
        <span class="ftab-badge">{Math.min(groupUnread, 99)}</span>
      {/if}
    </button>
  </div>

  <!-- Saved Messages — local-only personal notebook -->
  <ul class="saved-row">
    <li class:active={$activeConv === '__saved__'}>
      <button class="conv-btn" on:click={() => { conversations.update(c => ({ ...c, __saved__: c.__saved__ ?? [] })); activeConv.set('__saved__') }}>
        <span class="saved-icon">🔖</span>
        <span class="peer-name">Saved Messages</span>
      </button>
    </li>
  </ul>

  {#if folder === 'all' || folder === 'dms'}
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
      {#if filtered.length === 0 && folder === 'dms'}
        <li class="folder-empty">No DMs yet — start one above</li>
      {/if}
    </ul>
  {/if}

  {#if folder === 'all' || folder === 'groups'}
  {#if Object.keys($groups).length > 0 || showNewGroup}
    <div class="section-header">
      <span class="section-label">Groups</span>
      <button class="icon-btn" title="New group" on:click={() => showNewGroup = !showNewGroup}>#</button>
    </div>
    <ul>
      {#each Object.values($groups) as g}
        {@const unread = $unreadCounts[g.group_id] ?? 0}
        {@const groupChannels = $channels[g.group_id] ?? []}
        {@const expanded = channelExpandedGroups[g.group_id] ?? false}
        <li class:active={$activeConv === g.group_id}>
          <button class="conv-btn" on:click={() => { conversations.update(c => ({ ...c, [g.group_id]: c[g.group_id] ?? [] })); activeConv.set(g.group_id) }}>
            <span class="group-icon">#</span>
            <span class="peer-name" class:bold={unread > 0}>{g.name}</span>
            {#if $mutedConvs[g.group_id]}
              <span class="muted-icon" title="Muted">🔕</span>
            {:else if unread > 0}
              <span class="badge">{unread > 99 ? '99+' : unread}</span>
            {/if}
            {#if groupChannels.length > 0 || canManageChannels(g.group_id)}
              <button
                class="expand-btn"
                title={expanded ? 'Hide channels' : 'Show channels'}
                on:click|stopPropagation={() => channelExpandedGroups = { ...channelExpandedGroups, [g.group_id]: !expanded }}
              >{expanded ? '▾' : '▸'}</button>
            {/if}
          </button>
        </li>

        {#if expanded}
          <li class="channel-list">
            {#each groupChannels as ch}
              {@const chanPeerId = `${g.group_id}/${ch.channel_id}`}
              <button
                class="channel-item"
                class:active={$activeConv === chanPeerId}
                on:click={() => openChannel(g.group_id, ch.channel_id)}
                title={ch.description ?? ch.name}
              >
                <span class="chan-hash">#</span>
                <span class="chan-name">{ch.name}</span>
                {#if !ch.subscribed}<span class="chan-unsub" title="Not subscribed">○</span>{/if}
              </button>
            {/each}
            {#if canManageChannels(g.group_id)}
              <button
                class="channel-add-btn"
                title="Add channel"
                on:click|stopPropagation={() => { showNewChannelFor = showNewChannelFor === g.group_id ? null : g.group_id; newChannelName = ''; newChannelDesc = ''; channelError = '' }}
              >+ channel</button>
            {/if}
          </li>
        {/if}

        {#if showNewChannelFor === g.group_id}
          <li class="channel-form-li">
            <form class="channel-form" on:submit|preventDefault={() => createChannel(g.group_id)}>
              <input type="text" bind:value={newChannelName} placeholder="channel-name" disabled={channelLoading} maxlength="50" />
              <input type="text" bind:value={newChannelDesc} placeholder="Description (optional)" disabled={channelLoading} maxlength="200" />
              <div class="channel-form-btns">
                <button type="submit" disabled={channelLoading || !newChannelName.trim()}>Create</button>
                <button type="button" on:click={() => { showNewChannelFor = null; channelError = '' }}>✕</button>
              </div>
              {#if channelError}<p class="err">{channelError}</p>{/if}
            </form>
          </li>
        {/if}

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
  {/if}

  <div class="backup-row">
    <button class="icon-btn" title="Manage devices" on:click={() => showDeviceManager = true}>🖥</button>
    <button class="icon-btn" title="Chat statistics" on:click={() => showChatStats = true}>📊</button>
    <button class="icon-btn" title="Backup identity" on:click={() => { showBackup = !showBackup; showRestore = false; backupStatus = '' }}>⬇</button>
    <button class="icon-btn" title="Restore identity" on:click={() => { showRestore = !showRestore; showBackup = false; backupStatus = '' }}>⬆</button>
    <button
      class="icon-btn"
      class:dnd-active={dndEnabled}
      title="Focus / Do Not Disturb"
      on:click={() => showDnd = !showDnd}
    >{dndEnabled ? '🔕' : '🌙'}</button>
  </div>

  {#if showDnd}
    <div class="dnd-panel">
      <div class="dnd-row">
        <label class="dnd-toggle">
          <input type="checkbox" bind:checked={dndEnabled} on:change={saveDnd} />
          <span>Do Not Disturb</span>
        </label>
      </div>
      {#if dndEnabled}
        <div class="dnd-row dnd-times">
          <label>From <input type="time" bind:value={dndFrom} on:change={saveDnd} /></label>
          <label>To <input type="time" bind:value={dndTo} on:change={saveDnd} /></label>
        </div>
        <p class="dnd-hint">Notifications silenced during quiet hours</p>
      {/if}
    </div>
  {/if}

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

{#if showChatStats}
  <ChatStats on:close={() => showChatStats = false} />
{/if}

{#if showSettings}
  <SettingsModal
    bind:theme
    bind:accent
    bind:dndEnabled
    bind:dndFrom
    bind:dndTo
    bind:screenProtected
    onClose={() => showSettings = false}
  />
{/if}

<style>
  aside {
    width: 220px;
    height: 100%;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    flex-shrink: 0;
    overflow: hidden;
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
  .avatar-edit-btn { background: none; border: none; padding: 0; cursor: pointer; border-radius: 50%; }
  .avatar-edit-btn:hover { opacity: 0.8; }
  .profile-form {
    display: flex; flex-wrap: wrap; gap: 5px;
    padding: 6px 10px 8px; border-bottom: 1px solid var(--border);
  }
  .profile-form input { flex: 1; min-width: 0; font-size: 12px; padding: 5px 8px; }
  .profile-form button { font-size: 12px; padding: 5px 8px; }
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
  .saved-icon { font-size: 16px; width: 32px; height: 32px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
  .saved-row { border-bottom: 1px solid var(--border); }
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

  /* ── Channels ─── */
  .expand-btn {
    background: none; border: none; color: var(--text-dim);
    padding: 0 2px; font-size: 10px; flex-shrink: 0; cursor: pointer;
    line-height: 1;
  }
  .channel-list {
    list-style: none; padding: 0 0 2px 20px;
    border-bottom: 1px solid var(--border-sub);
  }
  .channel-item {
    display: flex; align-items: center; gap: 4px; width: 100%;
    padding: 4px 8px; font-size: 12px; color: var(--text-muted);
    background: none; text-align: left; border-radius: 4px; cursor: pointer;
  }
  .channel-item:hover { background: var(--bg-hover); color: var(--text); }
  .channel-item.active { background: var(--bg-active); color: var(--text); font-weight: 600; }
  .chan-hash { color: var(--text-dim); font-size: 11px; flex-shrink: 0; }
  .chan-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chan-unsub { font-size: 10px; color: var(--text-dim); flex-shrink: 0; }
  .channel-add-btn {
    display: block; width: 100%; padding: 3px 8px;
    font-size: 11px; color: var(--text-dim); background: none; text-align: left;
    border-radius: 4px; cursor: pointer;
  }
  .channel-add-btn:hover { background: var(--bg-hover); color: var(--accent); }
  .channel-form-li { list-style: none; }
  .channel-form {
    padding: 5px 10px 7px;
    display: flex; flex-direction: column; gap: 4px;
    border-bottom: 1px solid var(--border);
  }
  .channel-form input { font-size: 12px; padding: 4px 7px; }
  .channel-form-btns { display: flex; gap: 4px; }
  .channel-form-btns button { flex: 1; font-size: 11px; padding: 4px; }

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

  /* ── Theme popover ──────────────────────────────────────────────────────── */
  .theme-wrap { position: relative; }
  .theme-menu {
    position: absolute;
    bottom: calc(100% + 6px);
    right: 0;
    z-index: 200;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    padding: 10px 12px;
    min-width: 184px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .tm-section { display: flex; flex-direction: column; gap: 6px; }
  .tm-label { font-size: 10px; font-weight: 600; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.05em; }
  .tm-pills { display: flex; gap: 4px; }
  .tm-pill {
    flex: 1;
    background: var(--bg-hover);
    color: var(--text-muted);
    font-size: 11px;
    padding: 4px 6px;
    border-radius: 6px;
    border: 1px solid var(--border);
    cursor: pointer;
    transition: background 0.12s, color 0.12s;
  }
  .tm-pill:hover { background: var(--bg-hover); color: var(--text); }
  .tm-pill.selected { background: var(--accent); color: #fff; border-color: var(--accent); }
  .accent-swatches { display: flex; gap: 6px; flex-wrap: wrap; }
  .accent-swatch {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid transparent;
    padding: 0;
    cursor: pointer;
    flex-shrink: 0;
    transition: transform 0.12s, border-color 0.12s;
    outline-offset: 2px;
  }
  .accent-swatch:hover { transform: scale(1.18); }
  .accent-swatch.selected { border-color: var(--text); transform: scale(1.18); }

  /* ── Folder tabs ────────────────────────────────────────────────────────── */
  .folder-tabs {
    display: flex;
    gap: 3px;
    padding: 5px 8px 6px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .ftab {
    flex: 1;
    background: none;
    color: var(--text-dim);
    font-size: 12px;
    padding: 4px 4px;
    border-radius: 6px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 3px;
    transition: background 0.12s, color 0.12s;
    border: none;
  }
  .ftab:hover { background: var(--bg-hover); color: var(--text-muted); }
  .ftab.active { background: var(--bg-active); color: var(--accent); font-weight: 600; }
  .ftab-badge {
    font-size: 9px;
    font-weight: 700;
    color: #fff;
    background: var(--accent);
    border-radius: 999px;
    padding: 0 4px;
    min-width: 14px;
    height: 14px;
    display: flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }
  .folder-empty {
    padding: 20px 12px;
    font-size: 12px;
    color: var(--text-dim);
    text-align: center;
    list-style: none;
    border-bottom: none;
  }
  .folder-empty:hover { background: none; }

  /* Icons-only collapsed mode (controlled by parent Chat.svelte) */
  aside.collapsed { width: 56px; }
  aside.collapsed .username,
  aside.collapsed .peer-name,
  aside.collapsed .search-wrap,
  aside.collapsed .new-chat,
  aside.collapsed .folder-tabs,
  aside.collapsed .ws-dot,
  aside.collapsed .section-header,
  aside.collapsed .saved-row span,
  aside.collapsed .backup-row :not(:first-child),
  aside.collapsed .group-form,
  aside.collapsed .dnd-panel { display: none; }
  aside.collapsed .me { justify-content: center; }
  aside.collapsed .me-actions { gap: 2px; }
  aside.collapsed li { justify-content: center; padding: 10px 0; }
  aside.collapsed .conv-btn { justify-content: center; padding: 10px 0; }
  aside.collapsed .backup-row { flex-direction: column; gap: 6px; }

  /* ── DND panel ───────────────────────────────────────────────────────────── */
  .protected-on { color: var(--accent) !important; }
  .dnd-active { color: var(--accent) !important; }
  .dnd-panel {
    padding: 6px 10px 8px;
    border-top: 1px solid var(--border);
    display: flex; flex-direction: column; gap: 6px;
  }
  .dnd-row { display: flex; align-items: center; gap: 8px; font-size: 12px; }
  .dnd-toggle { display: flex; align-items: center; gap: 6px; cursor: pointer; color: var(--text); }
  .dnd-toggle input[type="checkbox"] { cursor: pointer; }
  .dnd-times { gap: 10px; flex-wrap: wrap; }
  .dnd-times label { display: flex; align-items: center; gap: 4px; color: var(--text-muted); font-size: 11px; }
  .dnd-times input[type="time"] { font-size: 11px; padding: 2px 4px; background: var(--bg-hover); border: 1px solid var(--border); color: var(--text); border-radius: 4px; }
  .dnd-hint { font-size: 10px; color: var(--text-dim); margin: 0; }
</style>
