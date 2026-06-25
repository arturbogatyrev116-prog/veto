<script>
  import { invoke } from '@tauri-apps/api/core'
  import Avatar from './Avatar.svelte'
  import { conversations, groups, user, peerNames } from '../stores.js'

  export let peerId
  export let onClose = null

  let tab = 'info'

  $: isGroup = peerId && Object.values($groups).some(g => g.group_id === peerId)
  $: isSaved = peerId === $user?.user_id
  $: groupInfo = $groups[peerId]
  $: peerName = isGroup
    ? (groupInfo?.name ?? peerId.slice(0, 8))
    : isSaved
    ? 'Saved Messages'
    : peerId

  // Collect media and links from messages
  $: messages = $conversations[peerId] ?? []
  $: mediaMessages = messages.filter(m => m.file_mime?.startsWith('image/') || m.file_mime?.startsWith('video/'))
  $: linkMessages = (() => {
    const urlRe = /https?:\/\/[^\s<>"]+/gi
    const seen = new Set()
    const out = []
    for (const m of messages) {
      if (!m.text) continue
      for (const url of (m.text.match(urlRe) ?? [])) {
        if (!seen.has(url)) { seen.add(url); out.push({ url, ts: m.ts, from: m.from }) }
      }
    }
    return out.reverse()
  })()

  // My role in this group
  $: myRole = groupInfo?.members?.find(m => m.user_id === $user?.user_id)?.role ?? 'member'
  $: myRoleRank = roleRank(myRole)

  function roleRank(r) {
    return { owner: 4, admin: 3, moderator: 2, member: 1 }[r] ?? 0
  }
  function roleBadge(r) {
    return { owner: '👑', admin: '⭐', moderator: '🔰', member: '' }[r] ?? ''
  }

  let memberMenuTarget = null  // { member, x, y }
  function openMemberMenu(e, member) {
    e.stopPropagation()
    memberMenuTarget = { member, x: e.clientX, y: e.clientY }
  }
  function closeMemberMenu() { memberMenuTarget = null }

  async function promoteToAdmin(m) {
    closeMemberMenu()
    await invoke('set_member_role', { groupId: peerId, userId: m.user_id, role: 'admin' }).catch(console.error)
    groups.update(gs => {
      const g = gs[peerId]; if (!g) return gs
      return { ...gs, [peerId]: { ...g, members: g.members.map(mb => mb.user_id === m.user_id ? { ...mb, role: 'admin' } : mb) } }
    })
  }
  async function promoteToModerator(m) {
    closeMemberMenu()
    await invoke('set_member_role', { groupId: peerId, userId: m.user_id, role: 'moderator' }).catch(console.error)
    groups.update(gs => {
      const g = gs[peerId]; if (!g) return gs
      return { ...gs, [peerId]: { ...g, members: g.members.map(mb => mb.user_id === m.user_id ? { ...mb, role: 'moderator' } : mb) } }
    })
  }
  async function demoteToMember(m) {
    closeMemberMenu()
    await invoke('set_member_role', { groupId: peerId, userId: m.user_id, role: 'member' }).catch(console.error)
    groups.update(gs => {
      const g = gs[peerId]; if (!g) return gs
      return { ...gs, [peerId]: { ...g, members: g.members.map(mb => mb.user_id === m.user_id ? { ...mb, role: 'member' } : mb) } }
    })
  }
  async function kickMember(m) {
    closeMemberMenu()
    if (!confirm(`Kick ${m.username}?`)) return
    await invoke('kick_member', { groupId: peerId, userId: m.user_id }).catch(console.error)
    groups.update(gs => {
      const g = gs[peerId]; if (!g) return gs
      return { ...gs, [peerId]: { ...g, members: g.members.filter(mb => mb.user_id !== m.user_id) } }
    })
  }
  async function transferOwnership(m) {
    closeMemberMenu()
    if (!confirm(`Transfer group ownership to ${m.username}? You will become admin.`)) return
    await invoke('transfer_ownership', { groupId: peerId, newOwnerId: m.user_id }).catch(console.error)
    groups.update(gs => {
      const g = gs[peerId]; if (!g) return gs
      return { ...gs, [peerId]: { ...g, members: g.members.map(mb => {
        if (mb.user_id === $user?.user_id) return { ...mb, role: 'admin' }
        if (mb.user_id === m.user_id) return { ...mb, role: 'owner' }
        return mb
      })}}
    })
  }

  async function clearHistory() {
    if (!confirm(`Delete all local messages with ${peerName}? This cannot be undone.`)) return
    try {
      await invoke('delete_message', { peerId, ts: 0, from: '', deleteAll: true })
    } catch (e) {
      console.error(e)
    }
  }

  function formatDate(ts) {
    return new Date(ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })
  }
</script>

<div class="detail-panel">
  <div class="detail-header">
    <span class="detail-title">Info</span>
    <button class="close-btn" on:click={onClose} aria-label="Close panel">✕</button>
  </div>

  <!-- Tabs -->
  <div class="tabs" role="tablist">
    <button role="tab" class:active={tab === 'info'}    on:click={() => tab = 'info'}>Info</button>
    <button role="tab" class:active={tab === 'media'}   on:click={() => tab = 'media'}>Media</button>
    <button role="tab" class:active={tab === 'links'}   on:click={() => tab = 'links'}>Links</button>
    {#if isGroup}
      <button role="tab" class:active={tab === 'members'} on:click={() => tab = 'members'}>Members</button>
    {/if}
  </div>

  <div class="tab-content">

    <!-- ── Info tab ── -->
    {#if tab === 'info'}
      <div class="info-tab">
        <div class="info-avatar">
          {#if isGroup}
            <div class="group-avatar-lg">#</div>
          {:else}
            <Avatar name={peerName} uid={peerId} size={72} />
          {/if}
        </div>
        <div class="info-name">{peerName}</div>
        {#if isGroup}
          <div class="info-sub">{groupInfo?.members?.length ?? 0} members</div>
        {:else if !isSaved}
          <div class="info-sub" title={peerId}>{peerId.slice(0, 8)}…</div>
        {/if}

        <div class="info-stats">
          <div class="stat-item">
            <span class="stat-val">{messages.length}</span>
            <span class="stat-lbl">Messages</span>
          </div>
          <div class="stat-item">
            <span class="stat-val">{mediaMessages.length}</span>
            <span class="stat-lbl">Media</span>
          </div>
          <div class="stat-item">
            <span class="stat-val">{linkMessages.length}</span>
            <span class="stat-lbl">Links</span>
          </div>
        </div>

        <div class="info-actions">
          <button class="danger-btn" on:click={clearHistory}>
            🗑 Clear local history
          </button>
        </div>
      </div>

    <!-- ── Media tab ── -->
    {:else if tab === 'media'}
      {#if mediaMessages.length === 0}
        <div class="empty-tab">No media yet</div>
      {:else}
        <div class="media-grid">
          {#each mediaMessages as m (m.ts)}
            <div class="media-thumb" title={formatDate(m.ts)}>
              {#if m.file_mime?.startsWith('image/')}
                {#if m.thumb_data}
                  <img src="data:image/jpeg;base64,{btoa(String.fromCharCode(...m.thumb_data))}" alt="" />
                {:else}
                  <div class="media-placeholder">🖼</div>
                {/if}
              {:else}
                <div class="media-placeholder">🎬</div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

    <!-- ── Links tab ── -->
    {:else if tab === 'links'}
      {#if linkMessages.length === 0}
        <div class="empty-tab">No links yet</div>
      {:else}
        <ul class="link-list">
          {#each linkMessages as l (l.url)}
            <li class="link-item">
              <a href={l.url} target="_blank" rel="noopener noreferrer" class="link-url">{l.url}</a>
              <span class="link-date">{formatDate(l.ts)}</span>
            </li>
          {/each}
        </ul>
      {/if}

    <!-- ── Members tab (groups only) ── -->
    {:else if tab === 'members'}
      {#if groupInfo?.members?.length}
        <ul class="member-list">
          {#each groupInfo.members.slice().sort((a,b) => roleRank(b.role) - roleRank(a.role)) as m (m.user_id)}
            {@const isMe = m.user_id === $user?.user_id}
            {@const canManage = !isMe && myRoleRank > roleRank(m.role)}
            <li class="member-item" class:manageable={canManage}>
              <Avatar name={m.username} uid={m.user_id} size={32} />
              <span class="member-name">{m.username}</span>
              {#if roleBadge(m.role)}<span class="role-badge" title={m.role}>{roleBadge(m.role)}</span>{/if}
              {#if isMe}
                <span class="member-you">you</span>
              {:else if canManage}
                <button class="member-menu-btn" on:click={e => openMemberMenu(e, m)} title="Manage">⋯</button>
              {/if}
            </li>
          {/each}
        </ul>
      {:else}
        <div class="empty-tab">No members</div>
      {/if}
    {/if}

  </div>
</div>

<!-- Member context menu -->
{#if memberMenuTarget}
  {@const tm = memberMenuTarget.member}
  {@const tr = tm.role}
  <div
    class="member-ctx-overlay"
    role="none"
    on:click={closeMemberMenu}
    on:keydown={e => e.key === 'Escape' && closeMemberMenu()}
  ></div>
  <div class="member-ctx-menu" style="top:{memberMenuTarget.y}px;left:{memberMenuTarget.x}px">
    {#if myRole === 'owner' && tr !== 'admin'}
      <button on:click={() => promoteToAdmin(tm)}>⭐ Make admin</button>
    {/if}
    {#if (myRole === 'owner' || myRole === 'admin') && tr === 'member'}
      <button on:click={() => promoteToModerator(tm)}>🔰 Make moderator</button>
    {/if}
    {#if (myRole === 'owner' || myRole === 'admin') && tr === 'moderator'}
      <button on:click={() => demoteToMember(tm)}>↓ Demote to member</button>
    {/if}
    {#if myRole === 'owner' && tr !== 'owner'}
      <button on:click={() => transferOwnership(tm)}>👑 Transfer ownership</button>
    {/if}
    {#if myRoleRank > roleRank(tr)}
      <button class="danger" on:click={() => kickMember(tm)}>🚫 Kick</button>
    {/if}
  </div>
{/if}

<style>
  .detail-panel {
    width: 280px;
    border-left: 1px solid var(--border);
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    flex-shrink: 0;
  }

  .detail-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .detail-title { font-weight: 600; font-size: 14px; color: var(--text); }
  .close-btn {
    background: none;
    color: var(--text-muted);
    font-size: 14px;
    padding: 2px 5px;
    border-radius: 5px;
    line-height: 1;
  }
  .close-btn:hover { background: var(--bg-hover); color: var(--text); }

  /* ── Tabs ── */
  .tabs {
    display: flex;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .tabs button {
    flex: 1;
    background: none;
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 500;
    padding: 8px 4px;
    border-radius: 0;
    border-bottom: 2px solid transparent;
    transition: color 0.15s, border-color 0.15s;
  }
  .tabs button:hover { color: var(--text); }
  .tabs button.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .tab-content {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
  }

  /* ── Info tab ── */
  .info-tab { display: flex; flex-direction: column; align-items: center; gap: 6px; }
  .info-avatar { margin-top: 8px; }
  .group-avatar-lg {
    width: 72px; height: 72px; border-radius: 50%;
    background: var(--bg-hover);
    display: flex; align-items: center; justify-content: center;
    font-size: 28px; color: var(--text-dim);
  }
  .info-name { font-weight: 700; font-size: 16px; color: var(--text); text-align: center; margin-top: 4px; }
  .info-sub { font-size: 12px; color: var(--text-dim); font-family: monospace; }

  .info-stats {
    display: flex; gap: 16px; margin: 12px 0 4px;
    background: var(--bg); border: 1px solid var(--border); border-radius: 10px;
    padding: 10px 14px; width: 100%; justify-content: space-around;
  }
  .stat-item { display: flex; flex-direction: column; align-items: center; gap: 2px; }
  .stat-val { font-weight: 700; font-size: 18px; color: var(--text); }
  .stat-lbl { font-size: 10px; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.04em; }

  .info-actions { width: 100%; margin-top: 8px; }
  .danger-btn {
    width: 100%;
    background: none;
    border: 1px solid var(--border);
    color: var(--danger);
    font-size: 13px;
    padding: 8px;
    border-radius: var(--radius);
    cursor: pointer;
    transition: background 0.12s;
  }
  .danger-btn:hover { background: color-mix(in srgb, var(--danger) 10%, transparent); }

  /* ── Media tab ── */
  .media-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 3px;
  }
  .media-thumb {
    aspect-ratio: 1;
    overflow: hidden;
    border-radius: 4px;
    background: var(--bg-hover);
    cursor: pointer;
  }
  .media-thumb img { width: 100%; height: 100%; object-fit: cover; }
  .media-placeholder {
    width: 100%; height: 100%;
    display: flex; align-items: center; justify-content: center;
    font-size: 22px;
  }

  /* ── Links tab ── */
  .link-list { list-style: none; display: flex; flex-direction: column; gap: 8px; }
  .link-item {
    display: flex; flex-direction: column; gap: 2px;
    padding: 8px; border: 1px solid var(--border); border-radius: 8px;
    background: var(--bg);
  }
  .link-url {
    font-size: 12px;
    color: var(--accent);
    word-break: break-all;
    text-decoration: none;
  }
  .link-url:hover { text-decoration: underline; }
  .link-date { font-size: 10px; color: var(--text-dim); }

  /* ── Members tab ── */
  .member-list { list-style: none; display: flex; flex-direction: column; gap: 2px; }
  .member-item {
    display: flex; align-items: center; gap: 10px;
    padding: 6px 4px;
    border-radius: 8px;
    transition: background 0.1s;
  }
  .member-item:hover { background: var(--bg-hover); }
  .member-name { flex: 1; font-size: 13px; color: var(--text); }
  .member-you {
    font-size: 10px; color: var(--accent);
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    padding: 1px 5px; border-radius: 10px;
  }
  .role-badge { font-size: 14px; }
  .member-menu-btn {
    background: none; color: var(--text-dim); font-size: 16px;
    padding: 0 4px; line-height: 1; opacity: 0;
    transition: opacity 0.1s;
  }
  .member-item.manageable:hover .member-menu-btn { opacity: 1; }
  .member-ctx-overlay {
    position: fixed; inset: 0; z-index: 200;
  }
  .member-ctx-menu {
    position: fixed; z-index: 201;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 4px 16px rgba(0,0,0,0.22);
    min-width: 170px;
    overflow: hidden;
    transform: translate(-50%, 8px);
  }
  .member-ctx-menu button {
    display: block; width: 100%;
    text-align: left; padding: 8px 14px;
    background: none; border-radius: 0;
    font-size: 13px; color: var(--text);
    transition: background 0.1s;
  }
  .member-ctx-menu button:hover { background: var(--bg-hover); }
  .member-ctx-menu button.danger { color: var(--danger); }

  /* ── Empty state ── */
  .empty-tab {
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
    padding: 32px 0;
  }
</style>
