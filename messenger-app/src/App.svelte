<script>
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { user, connLost, wsStatus, unlocked, activeConv, addMessage, addGroupMessage, typingPeers, conversations, onlinePeers, groups, showSearch, unreadCounts, reactions, peerNames } from './stores.js'
  import Register from './lib/Register.svelte'
  import PasswordPrompt from './lib/PasswordPrompt.svelte'
  import Chat from './lib/Chat.svelte'
  import SearchModal from './lib/SearchModal.svelte'
  import MediaBar from './lib/MediaBar.svelte'
  import { nowPlaying } from './stores.js'
  import './styles.css'

  let decryptErr = ''
  let ktWarning = ''
  let chatRef
  let updateInfo = null   // { version, notes } when an update is available
  let installing = false

  async function installUpdate() {
    installing = true
    try { await invoke('install_update') } catch (e) { console.error('update failed:', e); installing = false }
  }

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

  function playMention() {
    try {
      const ctx = new AudioContext()
      const gain = ctx.createGain()
      gain.connect(ctx.destination)
      ;[0, 0.15].forEach(offset => {
        const osc = ctx.createOscillator()
        osc.connect(gain)
        osc.frequency.value = 1100
        osc.start(ctx.currentTime + offset)
        osc.stop(ctx.currentTime + offset + 0.12)
      })
      gain.gain.setValueAtTime(0.18, ctx.currentTime)
      gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.35)
    } catch {}
  }

  function isDndActive() {
    if (localStorage.getItem('dnd_enabled') !== 'true') return false
    const from = localStorage.getItem('dnd_from')
    const to   = localStorage.getItem('dnd_to')
    if (!from || !to) return true  // enabled but no schedule = always DND
    const now = new Date()
    const nowM = now.getHours() * 60 + now.getMinutes()
    const [fH, fM] = from.split(':').map(Number)
    const [tH, tM] = to.split(':').map(Number)
    const fromM = fH * 60 + fM
    const toM   = tH * 60 + tM
    // crosses midnight if fromM > toM
    return fromM <= toM ? (nowM >= fromM && nowM < toM) : (nowM >= fromM || nowM < toM)
  }

  function notifyIfAllowed(senderName, text) {
    if (isDndActive()) return
    playNotify()
    if (!document.hasFocus()) {
      const body = text ? (text.length > 120 ? text.slice(0, 117) + '…' : text) : '📎 File'
      invoke('show_notification', { title: senderName, body }).catch(() => {})
    }
  }

  // Reconnect with exponential backoff: 1s → 2s → 4s → … → 30s
  let reconnectTimer = null
  let reconnectDelay = 1000

  async function attemptReconnect() {
    if (!$user || !$unlocked) return  // logged out or locked — don't reconnect
    wsStatus.set('reconnecting')
    try {
      await invoke('connect')
      wsStatus.set('connected')
      connLost.set(false)
      reconnectDelay = 1000  // reset backoff on success
    } catch {
      reconnectDelay = Math.min(reconnectDelay * 2, 30000)
      reconnectTimer = setTimeout(attemptReconnect, reconnectDelay)
    }
  }

  // When the user unlocks for the first time in this session, kick off the
  // initial connection attempt. PasswordPrompt no longer calls connect()
  // directly so the same retry loop handles both first connect and reconnects.
  let _didStartConnect = false
  $: if ($unlocked && !_didStartConnect) {
    _didStartConnect = true
    clearTimeout(reconnectTimer)
    reconnectDelay = 1000
    reconnectTimer = setTimeout(attemptReconnect, 0)
  }

  onMount(async () => {
    function onKeydown(e) {
      const ctrl = e.ctrlKey || e.metaKey
      // Ctrl+K — focus sidebar search
      if (ctrl && e.key === 'k') { e.preventDefault(); chatRef?.focusSearch?.() }
      // Ctrl+F — global message search
      if (ctrl && e.key === 'f') { e.preventDefault(); if ($unlocked) showSearch.set(true) }
      // Ctrl+N — new chat (focus sidebar input)
      if (ctrl && e.key === 'n') { e.preventDefault(); chatRef?.focusNewChat?.() }
      // Ctrl+Tab / Ctrl+Shift+Tab — next / previous conversation
      if (ctrl && e.key === 'Tab' && !e.shiftKey) { e.preventDefault(); chatRef?.nextConv?.() }
      if (ctrl && e.key === 'Tab' && e.shiftKey)  { e.preventDefault(); chatRef?.prevConv?.() }
      // Ctrl+W — close / go to no selection
      if (ctrl && e.key === 'w') { e.preventDefault(); if ($unlocked) activeConv.set(null) }
    }
    window.addEventListener('keydown', onKeydown)

    // Per-peer receipt debounce — at most one receipt per peer per 300 ms burst
    const receiptTimers = {}
    function sendReceiptDebounced(peerId) {
      clearTimeout(receiptTimers[peerId])
      receiptTimers[peerId] = setTimeout(() => {
        invoke('send_read_receipt', { peerId }).catch(() => {})
      }, 300)
    }

    // Register listeners BEFORE connect() so no events are missed
    const unlistenGroupMsg = await listen('group_message', ({ payload }) => {
      const { gid, from, text, ts, reply_to_ts, reply_to_from, reply_to_text,
              file_id, file_key, file_name, file_mime, file_size, thumb_data } = payload
      addGroupMessage(gid, {
        from,
        text,
        ts: ts ?? Date.now(),
        status: 'delivered',
        group_id: gid,
        sender_id: from,
        reply_to_ts: reply_to_ts ?? null,
        reply_to_from: reply_to_from ?? null,
        reply_to_text: reply_to_text ?? null,
        file_id: file_id ?? null,
        file_key: file_key ?? null,
        file_name: file_name ?? null,
        file_mime: file_mime ?? null,
        file_size: file_size ?? null,
        thumb_data: thumb_data ?? null,
      })
      let ac; activeConv.subscribe(v => { ac = v })()
      if (ac !== gid) {
        unreadCounts.update(c => ({ ...c, [gid]: (c[gid] ?? 0) + 1 }))
      }
      let grps; groups.subscribe(v => { grps = v })()
      const groupName = grps?.[gid]?.name ?? 'Group'
      notifyIfAllowed(groupName, text)
    })

    const unlistenMsg = await listen('message', ({ payload }) => {
      const { from, text, ts, reply_to_ts, reply_to_from, reply_to_text,
              file_id, file_key, file_name, file_mime, file_size, thumb_data } = payload
      addMessage(from, {
        from, text,
        ts: ts ?? Date.now(),
        status: 'delivered',
        reply_to_ts: reply_to_ts ?? null,
        reply_to_from: reply_to_from ?? null,
        reply_to_text: reply_to_text ?? null,
        file_id: file_id ?? null,
        file_key: file_key ?? null,
        file_name: file_name ?? null,
        file_mime: file_mime ?? null,
        file_size: file_size ?? null,
        thumb_data: thumb_data ?? null,
      })
      activeConv.update(c => c ?? from)
      // Send read receipt if this chat is currently open (debounced)
      let ac; activeConv.subscribe(v => { ac = v })()
      if (ac === from) {
        sendReceiptDebounced(from)
      } else {
        unreadCounts.update(c => ({ ...c, [from]: (c[from] ?? 0) + 1 }))
      }
      let names; peerNames.subscribe(v => { names = v })()
      const senderName = names?.[from] ?? (from.slice(0, 8) + '…')
      notifyIfAllowed(senderName, text)
    })

    const unlistenConn = await listen('connection_lost', () => {
      wsStatus.set('lost')
      connLost.set(true)
      onlinePeers.set(new Set())
      reconnectDelay = 1000
      reconnectTimer = setTimeout(attemptReconnect, reconnectDelay)
    })

    const unlistenErr = await listen('msg_error', ({ payload }) => {
      decryptErr = `Decrypt error from ${payload.sender?.slice(0, 8)}: ${payload.err}`
      setTimeout(() => { decryptErr = '' }, 5000)
    })

    const unlistenDelivered = await listen('delivered', ({ payload }) => {
      const { id, peerId } = payload
      conversations.update(convs => {
        const msgs = (convs[peerId] ?? []).map(m =>
          m.id === id ? { ...m, status: 'delivered' } : m
        )
        return { ...convs, [peerId]: msgs }
      })
    })

    // Peer read all our messages → blue ✓✓
    const unlistenRead = await listen('read', ({ payload }) => {
      const { from } = payload
      conversations.update(convs => {
        const msgs = (convs[from] ?? []).map(m =>
          m.from !== from ? { ...m, status: 'read' } : m
        )
        return { ...convs, [from]: msgs }
      })
    })

    // Initial online list when WS connects; also refresh group list and unread counts
    const unlistenHello = await listen('hello', ({ payload }) => {
      onlinePeers.set(new Set(payload.online_users ?? []))
      invoke('load_groups').then(gList => {
        groups.set(Object.fromEntries(gList.map(g => [g.group_id, g])))
      }).catch(() => {})
      invoke('get_unread_counts').then(counts => {
        unreadCounts.set(counts)
      }).catch(() => {})
    })

    // Live presence updates
    const unlistenReaction = await listen('reaction', ({ payload }) => {
      const { peer_id, msg_ts, msg_from, reactor_id, emoji, add } = payload
      const key = `${msg_ts}_${msg_from}`
      reactions.update(all => {
        const conv = { ...(all[peer_id] ?? {}) }
        if (add) {
          const msg = { ...(conv[key] ?? {}) }
          msg[emoji] = [...(msg[emoji] ?? []).filter(r => r !== reactor_id), reactor_id]
          conv[key] = msg
        } else {
          if (conv[key]) {
            const msg = { ...conv[key] }
            if (msg[emoji]) {
              msg[emoji] = msg[emoji].filter(r => r !== reactor_id)
              if (msg[emoji].length === 0) delete msg[emoji]
            }
            conv[key] = msg
          }
        }
        return { ...all, [peer_id]: conv }
      })
    })

    const unlistenUpdate = await listen('update_available', ({ payload }) => {
      updateInfo = payload
    })

    const unlistenPresence = await listen('presence', ({ payload }) => {
      const { user_id, online } = payload
      onlinePeers.update(s => {
        const next = new Set(s)
        if (online) next.add(user_id)
        else next.delete(user_id)
        return next
      })
    })

    // Typing indicators — auto-expire after 4 s
    const typingTimers = {}
    const unlistenTyping = await listen('typing', ({ payload }) => {
      const { from } = payload
      typingPeers.update(t => ({ ...t, [from]: true }))
      clearTimeout(typingTimers[from])
      typingTimers[from] = setTimeout(() => {
        typingPeers.update(t => { const n = { ...t }; delete n[from]; return n })
      }, 4000)
    })

    const unlistenMemberLeft = await listen('group_member_left', ({ payload }) => {
      const { groupId, userId } = payload
      groups.update(gs => {
        const g = gs[groupId]
        if (!g) return gs
        return { ...gs, [groupId]: { ...g, members: g.members.filter(m => m.user_id !== userId) } }
      })
    })

    const unlistenMention = await listen('mention', ({ payload }) => {
      if (isDndActive()) return
      const { peer_id, from, text } = payload
      playMention()
      if (!document.hasFocus()) {
        const senderName = $peerNames[from] ?? (from ?? '').slice(0, 8)
        const body = text ? (text.length > 120 ? text.slice(0, 117) + '…' : text) : 'You were mentioned'
        invoke('show_notification', { title: `@mention from ${senderName}`, body }).catch(() => {})
      }
    })

    const unlistenKt = await listen('kt_warning', ({ payload }) => {
      const { userId, reason } = payload
      ktWarning = `Security warning: key changed for ${(userId ?? '').slice(0, 8)}... — ${reason}`
      setTimeout(() => { ktWarning = '' }, 12000)
    })

    try {
      const info = await invoke('load_identity')
      if (info) {
        user.set(info)
        // Don't connect yet — PasswordPrompt will call unlock() then connect()
      }
    } catch (e) {
      console.error('load_identity failed:', e)
    }

    return () => {
      window.removeEventListener('keydown', onKeydown)
      clearTimeout(reconnectTimer)
      unlistenMsg(); unlistenGroupMsg(); unlistenConn(); unlistenErr(); unlistenDelivered(); unlistenTyping()
      unlistenRead(); unlistenHello(); unlistenPresence(); unlistenReaction(); unlistenUpdate()
      unlistenMemberLeft(); unlistenKt(); unlistenMention()
      Object.values(typingTimers).forEach(clearTimeout)
      Object.values(receiptTimers).forEach(clearTimeout)
    }
  })
</script>

{#if updateInfo}
  <div class="banner update-banner">
    <span>Update {updateInfo.version} available{updateInfo.notes ? ` — ${updateInfo.notes}` : ''}</span>
    <div class="update-actions">
      <button class="update-btn install" on:click={installUpdate} disabled={installing}>
        {installing ? 'Installing…' : 'Install & Restart'}
      </button>
      <button class="update-btn later" on:click={() => updateInfo = null}>Later</button>
    </div>
  </div>
{/if}
{#if $connLost}
  <div class="banner">
    {$wsStatus === 'reconnecting' ? 'Reconnecting…' : 'Connection lost. Reconnecting…'}
  </div>
{/if}
{#if decryptErr}
  <div class="banner err-banner">{decryptErr}</div>
{/if}
{#if ktWarning}
  <div class="banner kt-banner" role="alert">
    ⚠ {ktWarning}
    <button class="banner-close" on:click={() => ktWarning = ''}>×</button>
  </div>
{/if}

<MediaBar />

<div class="app-root" class:media-bar-open={$nowPlaying}>
  {#if $user && $unlocked}
    <Chat bind:this={chatRef} />
  {:else if $user}
    <PasswordPrompt />
  {:else}
    <Register />
  {/if}
</div>

<SearchModal />
