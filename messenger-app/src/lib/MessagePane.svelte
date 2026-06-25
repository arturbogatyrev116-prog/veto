<script>
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { tick, onMount, onDestroy } from 'svelte'
  import { marked } from 'marked'
  import hljs from 'highlight.js'
  import { conversations, activeConv, user, peerNames, typingPeers, addMessage, addGroupMessage, removeMessage, onlinePeers, unlocked, groups, unreadCounts, reactions } from '../stores.js'
  import Avatar from './Avatar.svelte'
  import SafetyNumbers from './SafetyNumbers.svelte'
  import AudioPlayer from './AudioPlayer.svelte'
  import VideoPlayer from './VideoPlayer.svelte'

  export let peerId

  let text = ''
  let error = ''
  let sending = false

  // ── Draft autosave ─────────────────────────────────────────────────────────

  let prevPeerId = null
  let draftTimer = null

  function saveDraft(pid, val) {
    invoke('save_draft', { peerId: pid, text: val }).catch(() => {})
  }

  function scheduleDraftSave(pid, val) {
    clearTimeout(draftTimer)
    draftTimer = setTimeout(() => saveDraft(pid, val), 500)
  }

  // When peerId changes, flush the old draft, then load the new peer's draft
  $: if (peerId !== prevPeerId && $unlocked) {
    if (prevPeerId) saveDraft(prevPeerId, text)
    prevPeerId = peerId
    invoke('get_draft', { peerId }).then(d => { text = d }).catch(() => {})
  }

  // ── Mute settings ──────────────────────────────────────────────────────────

  let muteSettings = { notifications_enabled: true, mute_until: 0, is_muted: false }
  let showMuteMenu = false

  $: if (peerId && $unlocked) {
    invoke('get_mute', { peerId }).then(s => { muteSettings = s }).catch(() => {})
  }

  async function setMute(hours) {
    showMuteMenu = false
    await invoke('set_mute', { peerId, muteHours: hours }).catch(() => {})
    muteSettings = await invoke('get_mute', { peerId }).catch(() => muteSettings)
  }

  // ── TTL (disappearing messages) ────────────────────────────────────────────

  let ttl = 0
  let showTtlMenu = false
  let expiringCount = 0
  let expiryCheckTimer = null

  $: if (peerId && $unlocked) {
    invoke('get_ttl', { peerId }).then(t => { ttl = t }).catch(() => {})
  }

  async function setTtl(secs) {
    showTtlMenu = false
    await invoke('set_ttl', { peerId, ttlSeconds: secs }).catch(() => {})
    ttl = secs
  }

  function checkExpiring() {
    if (ttl <= 0) { expiringCount = 0; return }
    const now = Date.now()
    const oneHour = 3_600_000
    expiringCount = ($conversations[peerId] ?? []).filter(m => {
      const exp = m.ts + ttl * 1000
      return exp > now && exp < now + oneHour
    }).length
  }

  onMount(() => {
    expiryCheckTimer = setInterval(checkExpiring, 60_000)
  })
  onDestroy(() => clearInterval(expiryCheckTimer))

  // ── Message editing ────────────────────────────────────────────────────────

  // Use ts+from as the edit key — db_id can be null for newly-sent messages,
  // which would cause null===null to match ALL messages without a db_id.
  let editingMsgKey = null
  let editText = ''
  let showEditHistory = null  // { msgId, history: [{old_plain, edited_at}] }

  function startEdit(m) {
    editingMsgKey = `${m.ts}_${m.from}`
    editText = m.text ?? ''
  }

  function cancelEdit() {
    editingMsgKey = null
    editText = ''
  }

  async function saveEdit(m) {
    const trimmed = editText.trim()
    if (!trimmed || trimmed === (m.text ?? '')) { cancelEdit(); return }
    try {
      await invoke('edit_message', { peerId, msgTs: m.ts, newText: trimmed })
      // Optimistically update local store — match by ts+from, not db_id (which may be null)
      conversations.update(convs => {
        const msgs = (convs[peerId] ?? []).map(msg =>
          (msg.ts === m.ts && msg.from === m.from) ? { ...msg, text: trimmed, edited_at: Date.now() } : msg
        )
        return { ...convs, [peerId]: msgs }
      })
    } catch (e) {
      error = String(e)
    }
    cancelEdit()
  }

  async function showHistory(m) {
    const history = await invoke('get_edit_history', { msgId: m.db_id }).catch(() => [])
    showEditHistory = { msgId: m.db_id, history }
  }

  // Listen for incoming edits from peers
  let unlistenEdit
  onMount(async () => {
    unlistenEdit = await listen('message_edited', ({ payload }) => {
      const { peer_id, msg_id, msg_ts, new_text, edited_at } = payload
      conversations.update(convs => {
        const msgs = (convs[peer_id] ?? []).map(m =>
          // match by db_id if available (DB-loaded), else by ts (wire messages in current session)
          (m.db_id != null ? m.db_id === msg_id : m.ts === msg_ts)
            ? { ...m, text: new_text, edited_at }
            : m
        )
        return { ...convs, [peer_id]: msgs }
      })
    })
  })
  onDestroy(() => { unlistenEdit?.() })

  // Listen for incoming deletes from peers
  let unlistenDel
  onMount(async () => {
    unlistenDel = await listen('message_deleted', ({ payload }) => {
      const { peer_id, msg_ts, msg_from } = payload
      removeMessage(peer_id, msg_ts, msg_from)
    })
  })
  onDestroy(() => { unlistenDel?.() })

  // ── Group read receipts ────────────────────────────────────────────────────
  // { [user_id]: ts_ms } — read watermark per member for the current group
  let groupReadMarks = {}

  let unlistenGrpRead
  onMount(async () => {
    unlistenGrpRead = await listen('group_read', ({ payload }) => {
      if (payload.gid === peerId) {
        groupReadMarks = { ...groupReadMarks, [payload.from]: payload.ts }
      }
    })
  })
  onDestroy(() => { unlistenGrpRead?.() })

  // ── Chat export ────────────────────────────────────────────────────────────

  let showExport = false
  let exportFormat = 'markdown'
  let exportEncrypted = false
  let exportPassword = ''
  let exporting = false

  async function doExport() {
    if (exporting) return
    exporting = true
    error = ''
    try {
      const bytes = await invoke('export_chat', {
        peerId,
        format: exportFormat,
        encrypted: exportEncrypted,
        password: exportEncrypted ? exportPassword : null,
      })
      const ext = exportFormat === 'html' ? 'html' : exportFormat === 'json' ? 'json' : 'md'
      const filename = `chat-${peerId.slice(0, 8)}-${Date.now()}.${ext}${exportEncrypted ? '.cexp' : ''}`
      const blob = new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url; a.download = filename; a.click()
      setTimeout(() => URL.revokeObjectURL(url), 10_000)
      showExport = false
      exportPassword = ''
    } catch (e) {
      error = String(e)
    } finally {
      exporting = false
    }
  }
  let messagesEl
  let textareaEl
  let showSafetyNumbers = false
  let dragging = false
  function onDragOver(e) { e.preventDefault(); dragging = true }
  function onDragLeave(e) { if (!e.currentTarget.contains(e.relatedTarget)) dragging = false }
  async function onDrop(e) {
    e.preventDefault(); dragging = false
    const files = [...(e.dataTransfer?.files ?? [])]
    for (const f of files) await sendFile(f)
  }
  let showScrollBtn = false
  let userScrolledUp = false
  // Reply-to state: { ts, from, peerName, text } | null
  let replyTo = null

  // Lightbox: URL of full-res image to show, or null
  let lightboxUrl = null

  // LRU blob URL cache for thumbnails — max 20 entries, oldest evicted first
  const THUMB_CACHE_MAX = 20
  const thumbCache = new Map()

  function getThumbnailUrl(msgKey, thumbData) {
    if (!thumbData?.length) return null
    if (thumbCache.has(msgKey)) return thumbCache.get(msgKey)
    if (thumbCache.size >= THUMB_CACHE_MAX) {
      const oldest = thumbCache.keys().next().value
      URL.revokeObjectURL(thumbCache.get(oldest))
      thumbCache.delete(oldest)
    }
    const blob = new Blob([new Uint8Array(thumbData)], { type: 'image/jpeg' })
    const url = URL.createObjectURL(blob)
    thumbCache.set(msgKey, url)
    return url
  }

  async function generateThumbnail(file) {
    if (!file.type.startsWith('image/')) return null
    try {
      const bitmap = await createImageBitmap(file, { imageOrientation: 'from-image' })
      const maxDim = 200
      const scale = Math.min(maxDim / bitmap.width, maxDim / bitmap.height, 1)
      const w = Math.round(bitmap.width * scale) || 1
      const h = Math.round(bitmap.height * scale) || 1
      const canvas = new OffscreenCanvas(w, h)
      canvas.getContext('2d').drawImage(bitmap, 0, 0, w, h)
      bitmap.close()
      const blob = await canvas.convertToBlob({ type: 'image/jpeg', quality: 0.82 })
      return Array.from(new Uint8Array(await blob.arrayBuffer()))
    } catch {
      return null
    }
  }

  const PAGE_SIZE = 50
  // Load history from SQLite when conversation opens (once per peer per session).
  const loadedPeers = new Set()
  // { [peerId]: { oldestDbId: number|null, hasMore: bool } }
  let peerMeta = {}
  let loadingMore = false
  // { [peerId]: number } — timestamp up to which messages are considered read
  let lastReadByPeer = {}
  // Per-peer mark-as-read timer (fires 1s after opening a conversation)
  let readTimers = {}

  // Reset group read marks when switching conversations
  $: if (peerId) { groupReadMarks = {} }

  $: if (peerId && $unlocked && !loadedPeers.has(peerId)) {
    loadedPeers.add(peerId)
    const isGrp = !!$groups[peerId]
    // Fetch last-read timestamp in parallel with history load
    invoke('get_last_read_ts', { peerId }).then(ts => {
      lastReadByPeer = { ...lastReadByPeer, [peerId]: ts }
    }).catch(() => {})
    // Load per-member read marks for group chats
    if (isGrp) {
      invoke('get_group_read_marks', { groupId: peerId })
        .then(marks => { groupReadMarks = marks })
        .catch(() => {})
    }
    const loadPromise = isGrp
      ? invoke('get_group_messages', { groupId: peerId, limit: PAGE_SIZE, beforeId: null })
      : invoke('get_messages', { peerId, limit: PAGE_SIZE, beforeId: null })
    loadPromise.then(history => {
      conversations.update(c => ({ ...c, [peerId]: history }))
      peerMeta[peerId] = {
        oldestDbId: history.length > 0 ? history[0].db_id : null,
        hasMore: history.length === PAGE_SIZE,
      }
      if (!isGrp && history.some(m => m.from === peerId)) {
        invoke('send_read_receipt', { peerId }).catch(() => {})
      }
    }).catch(() => {})
  }

  // When switching to a conversation, schedule mark-as-read after 1 s
  $: if (peerId) {
    clearTimeout(readTimers[peerId])
    readTimers[peerId] = setTimeout(() => {
      const msgs = $conversations[peerId] ?? []
      const maxTs = msgs.reduce((acc, m) => m.ts > acc ? m.ts : acc, 0)
      if (maxTs > 0) {
        invoke('mark_as_read', { peerId, ts: maxTs }).catch(() => {})
        lastReadByPeer = { ...lastReadByPeer, [peerId]: maxTs }
        unreadCounts.update(c => { const n = { ...c }; delete n[peerId]; return n })
        if (!!$groups[peerId]) {
          invoke('send_group_read_receipt', { groupId: peerId, ts: maxTs }).catch(() => {})
        }
      }
    }, 1000)
  }

  // ── Reactions ──────────────────────────────────────────────────────────────

  const QUICK_EMOJIS = ['👍', '❤️', '😂', '😮', '😢', '🎉', '🔥', '👀']

  // Load reactions when conversation opens
  $: if (peerId && $unlocked && loadedPeers.has(peerId)) {
    invoke('get_reactions', { peerId }).then(rows => {
      reactions.update(all => {
        const conv = {}
        for (const r of rows) {
          const key = `${r.msg_ts}_${r.msg_from}`
          if (!conv[key]) conv[key] = {}
          if (!conv[key][r.emoji]) conv[key][r.emoji] = []
          if (!conv[key][r.emoji].includes(r.reactor_id)) {
            conv[key][r.emoji].push(r.reactor_id)
          }
        }
        return { ...all, [peerId]: conv }
      })
    }).catch(() => {})
  }

  // Picker visibility: { msgKey: true } — one picker open at a time
  let pickerVisible = {}
  let pickerTimers = {}

  function showPicker(msgKey) {
    clearTimeout(pickerTimers[msgKey])
    pickerVisible = { [msgKey]: true }
  }

  function hidePicker(msgKey) {
    pickerTimers[msgKey] = setTimeout(() => {
      pickerVisible = { ...pickerVisible }
      delete pickerVisible[msgKey]
      pickerVisible = pickerVisible
    }, 200)
  }

  function keepPicker(msgKey) {
    clearTimeout(pickerTimers[msgKey])
  }

  async function selectReaction(m, newEmoji) {
    const msgKey = `${m.ts}_${m.from}`
    pickerVisible = {}
    const convReactions = $reactions[peerId] ?? {}
    const myReactors = Object.entries(convReactions[msgKey] ?? {})
    const currentEmoji = myReactors.find(([, arr]) => arr.includes($user.user_id))?.[0]
    if (currentEmoji === newEmoji) {
      // Toggle off own reaction
      await invoke('send_reaction', { peerId, msgTs: m.ts, msgFrom: m.from, emoji: newEmoji, add: false }).catch(() => {})
      reactions.update(all => {
        const conv = { ...(all[peerId] ?? {}) }
        const msg = { ...(conv[msgKey] ?? {}) }
        msg[newEmoji] = (msg[newEmoji] ?? []).filter(r => r !== $user.user_id)
        if (msg[newEmoji].length === 0) delete msg[newEmoji]
        conv[msgKey] = msg
        return { ...all, [peerId]: conv }
      })
    } else {
      // Remove old reaction if exists
      if (currentEmoji) {
        await invoke('send_reaction', { peerId, msgTs: m.ts, msgFrom: m.from, emoji: currentEmoji, add: false }).catch(() => {})
      }
      // Add new reaction
      await invoke('send_reaction', { peerId, msgTs: m.ts, msgFrom: m.from, emoji: newEmoji, add: true }).catch(() => {})
      reactions.update(all => {
        const conv = { ...(all[peerId] ?? {}) }
        const msg = { ...(conv[msgKey] ?? {}) }
        // Remove from old emoji
        if (currentEmoji && msg[currentEmoji]) {
          msg[currentEmoji] = msg[currentEmoji].filter(r => r !== $user.user_id)
          if (msg[currentEmoji].length === 0) delete msg[currentEmoji]
        }
        // Add to new emoji
        msg[newEmoji] = [...(msg[newEmoji] ?? []).filter(r => r !== $user.user_id), $user.user_id]
        conv[msgKey] = msg
        return { ...all, [peerId]: conv }
      })
    }
  }

  function reactionTooltip(emoji, reactorIds) {
    return reactorIds.map(id => $peerNames[id] ?? id.slice(0, 8)).join(', ')
  }

  // Compute the index of the first unread received message (for divider placement)
  $: firstUnreadIdx = (() => {
    const msgs = $conversations[peerId] ?? []
    const lr = lastReadByPeer[peerId] ?? 0
    if (lr === 0) return -1  // no read state — no divider
    for (let i = 0; i < msgs.length; i++) {
      if (msgs[i].ts > lr && msgs[i].from !== ($user?.user_id ?? '')) return i
    }
    return -1
  })()

  async function loadMore() {
    if (!peerId || !peerMeta[peerId]?.hasMore || loadingMore) return
    const meta = peerMeta[peerId]
    const prevScrollHeight = messagesEl?.scrollHeight ?? 0
    const prevScrollTop  = messagesEl?.scrollTop  ?? 0
    loadingMore = true
    try {
      const more = await (isGroup
        ? invoke('get_group_messages', { groupId: peerId, limit: PAGE_SIZE, beforeId: meta.oldestDbId })
        : invoke('get_messages', { peerId, limit: PAGE_SIZE, beforeId: meta.oldestDbId }))
      if (more.length > 0) {
        peerMeta[peerId] = { oldestDbId: more[0].db_id, hasMore: more.length === PAGE_SIZE }
        conversations.update(c => ({ ...c, [peerId]: [...more, ...(c[peerId] ?? [])] }))
        await tick()
        // Restore visual scroll position after prepending older messages.
        if (messagesEl) {
          messagesEl.scrollTop = messagesEl.scrollHeight - prevScrollHeight + prevScrollTop
        }
      } else {
        peerMeta[peerId] = { ...meta, hasMore: false }
      }
    } catch (_) {
      // ignore — leave hasMore as-is so the user can retry
    } finally {
      loadingMore = false
    }
  }

  function formatTs(ts) {
    if (!ts) return ''
    const d = new Date(ts)
    const now = new Date()
    if (d.toDateString() === now.toDateString()) {
      return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    }
    return d.toLocaleDateString([], { month: 'short', day: 'numeric' }) + ' ' +
           d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }

  // ── Pinned messages ──────────────────────────────────────────────────────────
  let pinnedMsgs = []
  let showPinnedList = false

  $: if (peerId && $unlocked && loadedPeers.has(peerId)) {
    invoke('get_pinned_messages', { peerId }).then(p => { pinnedMsgs = p }).catch(() => {})
  }
  $: if (peerId) { pinnedMsgs = []; showPinnedList = false }

  async function togglePin(m) {
    const isPinned = pinnedMsgs.some(p => p.msg_ts === m.ts && p.msg_from === m.from)
    if (isPinned) {
      await invoke('unpin_message', { peerId, msgTs: m.ts, msgFrom: m.from })
      pinnedMsgs = pinnedMsgs.filter(p => !(p.msg_ts === m.ts && p.msg_from === m.from))
    } else {
      await invoke('pin_message', { peerId, msgTs: m.ts, msgFrom: m.from, msgText: m.text ?? '' })
      pinnedMsgs = [...pinnedMsgs, { msg_ts: m.ts, msg_from: m.from, msg_text: m.text ?? '', pinned_at: Date.now() }]
    }
    ctxVisible = false
  }

  function scrollToPinned(pin) {
    const el = messagesEl?.querySelector(`[data-ts="${pin.msg_ts}"]`)
    if (el) { el.scrollIntoView({ behavior: 'smooth', block: 'center' }); showPinnedList = false }
  }

  // Context menu state
  let ctxVisible = false
  let ctxX = 0, ctxY = 0
  let ctxText = ''
  let ctxMsg = null
  let ctxMine = false

  const renderer = new marked.Renderer()
  renderer.code = ({ text, lang }) => {
    const language = lang && hljs.getLanguage(lang) ? lang : 'plaintext'
    const highlighted = hljs.highlight(text, { language }).value
    return `<pre><code class="hljs language-${language}">${highlighted}</code></pre>`
  }
  marked.setOptions({ breaks: true, gfm: true, renderer })

  function renderMd(raw) {
    return marked.parse(raw ?? '')
  }

  $: messages = $conversations[peerId] ?? []
  $: peerName = $peerNames[peerId] ?? peerId.slice(0, 8) + '…'
  $: isTyping = !!$typingPeers[peerId]
  $: isGroup = !!$groups[peerId]
  $: isSaved = peerId === '__saved__'
  $: groupInfo = $groups[peerId] ?? null

  // Whether this group uses Sender Keys (O(1) encryption)
  let groupUseSK = false
  $: if (isGroup && peerId && $unlocked) {
    invoke('group_has_sender_key', { groupId: peerId }).then(v => { groupUseSK = v }).catch(() => { groupUseSK = false })
  } else {
    groupUseSK = false
  }

  // Reset scroll state when conversation changes.
  $: if (peerId) { userScrolledUp = false; showScrollBtn = false }

  // Auto-scroll on new messages — skipped when user has scrolled up to read history.
  $: if (messagesEl && messages && !loadingMore && !userScrolledUp) tick().then(scrollBottom)

  function scrollBottom() {
    messagesEl?.scrollTo({ top: messagesEl.scrollHeight, behavior: 'smooth' })
  }

  function onScroll() {
    if (!messagesEl) return
    const { scrollTop, scrollHeight, clientHeight } = messagesEl
    userScrolledUp = scrollHeight - scrollTop - clientHeight > 100
    showScrollBtn = userScrolledUp
  }

  // ── File attachment ────────────────────────────────────────────────────────

  let fileInput
  let downloadingFiles = {}

  function formatFileSize(bytes) {
    if (!bytes) return ''
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  async function attachFile() {
    fileInput?.click()
  }

  async function onFileSelected(e) {
    const file = e.target.files?.[0]
    if (!file) return
    e.target.value = ''
    sending = true
    error = ''
    try {
      const fileBytes = Array.from(new Uint8Array(await file.arrayBuffer()))
      const mime = file.type || 'application/octet-stream'
      const thumbBytes = await generateThumbnail(file)
      if (isGroup) {
        const { file_id, file_key } = await invoke('send_group_file', {
          groupId: peerId,
          fileBytes,
          fileName: file.name,
          mimeType: mime,
          thumbBytes,
        })
        addGroupMessage(peerId, {
          from: $user.user_id,
          text: '',
          ts: Date.now(),
          status: 'sent',
          group_id: peerId,
          sender_id: $user.user_id,
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id, file_key,
          file_name: file.name, file_mime: mime, file_size: file.size,
          thumb_data: thumbBytes,
        })
      } else {
        const { id, file_id, file_key } = await invoke('send_file', {
          peerId,
          fileBytes,
          fileName: file.name,
          mimeType: mime,
          thumbBytes,
        })
        addMessage(peerId, {
          from: $user.user_id,
          text: '',
          ts: Date.now(),
          id,
          status: 'sent',
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id, file_key,
          file_name: file.name, file_mime: mime, file_size: file.size,
          thumb_data: thumbBytes,
        })
      }
      await tick()
      scrollBottom()
    } catch (e) {
      error = String(e)
    } finally {
      sending = false
    }
  }

  async function leaveGroup() {
    if (!confirm(`Leave group "${groupInfo?.name}"?`)) return
    try {
      await invoke('leave_group', { groupId: peerId })
      groups.update(g => { const n = { ...g }; delete n[peerId]; return n })
      conversations.update(c => { const n = { ...c }; delete n[peerId]; return n })
      activeConv.set(null)
    } catch (e) {
      error = String(e)
    }
  }

  async function downloadFile(m) {
    if (!m.file_key || !m.file_id) return
    downloadingFiles = { ...downloadingFiles, [m.file_id]: true }
    try {
      const bytes = await invoke('download_file', {
        fileId: m.file_id,
        keyBytes: m.file_key,
      })
      const blob = new Blob([new Uint8Array(bytes)], { type: m.file_mime || 'application/octet-stream' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = m.file_name || 'file'
      a.click()
      setTimeout(() => URL.revokeObjectURL(url), 10000)
    } catch (e) {
      error = String(e)
    } finally {
      downloadingFiles = { ...downloadingFiles, [m.file_id]: false }
    }
  }

  async function openImageLightbox(m) {
    if (!m.file_key || !m.file_id) return
    downloadingFiles = { ...downloadingFiles, [m.file_id]: true }
    try {
      const bytes = await invoke('download_file', {
        fileId: m.file_id,
        keyBytes: m.file_key,
      })
      const blob = new Blob([new Uint8Array(bytes)], { type: m.file_mime || 'image/jpeg' })
      lightboxUrl = URL.createObjectURL(blob)
    } catch (e) {
      error = String(e)
    } finally {
      downloadingFiles = { ...downloadingFiles, [m.file_id]: false }
    }
  }

  // ── Send text ──────────────────────────────────────────────────────────────

  async function send() {
    const msg = text.trim()
    if (!msg) return
    sending = true
    error = ''
    try {
      const replyArg = replyTo
        ? { ts: replyTo.ts, from: replyTo.from, text: replyTo.text }
        : null
      if (isSaved) {
        const ts = await invoke('save_note', { text: msg })
        addMessage(peerId, {
          from: $user.user_id,
          text: msg,
          ts,
          id: null,
          status: 'sent',
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null,
        })
      } else if (isGroup) {
        await invoke('send_group_message', { groupId: peerId, text: msg, replyTo: replyArg })
        addMessage(peerId, {
          from: $user.user_id,
          text: msg,
          ts: Date.now(),
          status: 'sent',
          group_id: peerId,
          sender_id: $user.user_id,
          reply_to_ts:   replyTo?.ts   ?? null,
          reply_to_from: replyTo?.from ?? null,
          reply_to_text: replyTo?.text ?? null,
          file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null,
        })
      } else {
        const { id, ts: msgTs } = await invoke('send_message', { peerId, text: msg, replyTo: replyArg })
        addMessage(peerId, {
          from: $user.user_id,
          text: msg,
          ts: msgTs,
          id,
          status: 'sent',
          reply_to_ts:   replyTo?.ts   ?? null,
          reply_to_from: replyTo?.from ?? null,
          reply_to_text: replyTo?.text ?? null,
          file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null,
        })
      }
      text = ''
      replyTo = null
      saveDraft(peerId, '')
      await tick()
      scrollBottom()
      textareaEl?.focus()
    } catch (e) {
      error = String(e)
    } finally {
      sending = false
    }
  }

  function onKeydown(e) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      send()
    }
  }

  // ── Typing indicator ───────────────────────────────────────────────────────

  let typingTimer = null

  function onInput() {
    scheduleDraftSave(peerId, text)
    if (isGroup) return  // Typing indicators are DM-only
    if (typingTimer) return
    invoke('send_typing', { peerId }).catch(() => {})
    typingTimer = setTimeout(() => { typingTimer = null }, 3000)
  }

  function onBlur() {
    // Clear debounce on blur so next focus triggers immediately
    clearTimeout(typingTimer)
    typingTimer = null
  }

  // ── Voice / Video recording ────────────────────────────────────────────────

  // recState: 'idle' | 'voice' | 'voice-locked' | 'video'
  let recState = 'idle'
  let mediaRecorder = null
  let recChunks = []
  let recStartY = 0
  let recStartX = 0
  let recSwipeDy = 0   // positive = up
  let recPressTimer = null
  const LONG_PRESS_MS = 300

  // Video modal
  let showVideoModal = false
  let videoPreviewEl = null
  let videoStream = null
  let videoRecording = false

  async function startVoice() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
      recChunks = []
      mediaRecorder = new MediaRecorder(stream, { mimeType: 'audio/webm;codecs=opus' })
      mediaRecorder.ondataavailable = e => { if (e.data.size > 0) recChunks.push(e.data) }
      mediaRecorder.start()
      recState = 'voice'
    } catch {
      recState = 'idle'
    }
  }

  async function stopAndSendVoice(cancel = false) {
    clearInterval(recTicker); recTicker = null; recSeconds = 0
    if (!mediaRecorder) { recState = 'idle'; return }
    if (cancel) {
      mediaRecorder.stream.getTracks().forEach(t => t.stop())
      mediaRecorder = null; recChunks = []; recState = 'idle'; return
    }
    // stop() triggers final ondataavailable — must await before checking chunks
    await new Promise(r => { mediaRecorder.onstop = r; mediaRecorder.stop() })
    mediaRecorder.stream.getTracks().forEach(t => t.stop())
    const blob = new Blob(recChunks, { type: 'audio/webm' })
    mediaRecorder = null; recChunks = []; recState = 'idle'
    if (blob.size > 0) await sendMediaBlob(blob, 'voice.webm', 'audio/webm')
  }

  let recSeconds = 0
  let recTicker = null

  function onMicPointerDown(e) {
    e.currentTarget.setPointerCapture(e.pointerId)  // track swipe outside button
    recStartY = e.clientY
    recStartX = e.clientX
    recSwipeDy = 0
    recPressTimer = setTimeout(() => {
      recPressTimer = null
      startVoice()
      recSeconds = 0
      recTicker = setInterval(() => recSeconds++, 1000)
    }, LONG_PRESS_MS)
  }

  function onMicPointerMove(e) {
    if (recState === 'idle') return
    const dy = recStartY - e.clientY   // positive = moved up
    const dx = recStartX - e.clientX   // positive = moved left
    recSwipeDy = dy
    if (recState === 'voice' && dy > 50) recState = 'voice-locked'
    if ((recState === 'voice' || recState === 'voice-locked') && dx > 100) {
      clearInterval(recTicker); recTicker = null
      stopAndSendVoice(true)
    }
  }

  function onMicPointerUp() {
    if (recPressTimer !== null) {
      // Short tap → open video
      clearTimeout(recPressTimer)
      recPressTimer = null
      openVideoModal()
      return
    }
    if (recState === 'voice') {
      clearInterval(recTicker); recTicker = null
      stopAndSendVoice(false)
    }
    // voice-locked: ignore pointerup, user stops via stop button
  }

  function fmtRecTime(s) {
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
  }

  async function openVideoModal() {
    try {
      videoStream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true })
      showVideoModal = true
      await tick()
      if (videoPreviewEl) { videoPreviewEl.srcObject = videoStream }
    } catch { videoStream = null }
  }

  function closeVideoModal(cancel = true) {
    if (videoRecording && mediaRecorder) stopAndSendVoice(cancel)
    videoStream?.getTracks().forEach(t => t.stop())
    videoStream = null
    videoRecording = false
    showVideoModal = false
    mediaRecorder = null
    recChunks = []
  }

  async function startVideoRec() {
    if (!videoStream) return
    recChunks = []
    const mime = MediaRecorder.isTypeSupported('video/webm;codecs=vp9,opus')
      ? 'video/webm;codecs=vp9,opus'
      : 'video/webm'
    mediaRecorder = new MediaRecorder(videoStream, { mimeType: mime })
    mediaRecorder.ondataavailable = e => { if (e.data.size > 0) recChunks.push(e.data) }
    mediaRecorder.start()
    videoRecording = true
  }

  async function stopVideoRec() {
    if (!mediaRecorder) return
    await new Promise(r => { mediaRecorder.onstop = r; mediaRecorder.stop() })
    const blob = new Blob(recChunks, { type: 'video/webm' })
    let thumbBytes = null
    try {
      // Grab first frame for thumbnail
      const url = URL.createObjectURL(blob)
      const vid = document.createElement('video')
      vid.src = url
      vid.muted = true
      await new Promise(r => { vid.onloadeddata = r; vid.load() })
      await new Promise(r => { vid.onseeked = r; vid.currentTime = 0.1 })
      const canvas = document.createElement('canvas')
      canvas.width = Math.min(vid.videoWidth, 320)
      canvas.height = Math.round(vid.videoHeight * canvas.width / vid.videoWidth)
      canvas.getContext('2d').drawImage(vid, 0, 0, canvas.width, canvas.height)
      const thumbBlob = await new Promise(r => canvas.toBlob(r, 'image/jpeg', 0.8))
      thumbBytes = Array.from(new Uint8Array(await thumbBlob.arrayBuffer()))
      URL.revokeObjectURL(url)
    } catch {}
    mediaRecorder = null; recChunks = []; videoRecording = false
    closeVideoModal(false)
    await sendMediaBlob(blob, 'video.webm', 'video/webm', thumbBytes)
  }

  async function sendMediaBlob(blob, fileName, mimeType, thumbBytes = null) {
    sending = true; error = ''
    try {
      const fileBytes = Array.from(new Uint8Array(await blob.arrayBuffer()))
      if (isGroup) {
        const { file_id, file_key } = await invoke('send_group_file', {
          groupId: peerId, fileBytes, fileName, mimeType, thumbBytes,
        })
        addGroupMessage(peerId, {
          from: $user.user_id, text: '', ts: Date.now(), status: 'sent',
          group_id: peerId, sender_id: $user.user_id,
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id, file_key, file_name: fileName, file_mime: mimeType,
          file_size: blob.size, thumb_data: thumbBytes,
        })
      } else {
        const { id, file_id, file_key } = await invoke('send_file', {
          peerId, fileBytes, fileName, mimeType, thumbBytes,
        })
        addMessage(peerId, {
          from: $user.user_id, text: '', ts: Date.now(), id, status: 'sent',
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id, file_key, file_name: fileName, file_mime: mimeType,
          file_size: blob.size, thumb_data: thumbBytes,
        })
      }
      await tick(); scrollBottom()
    } catch (e) { error = String(e) }
    finally { sending = false }
  }

  // ── Auto-resize textarea ───────────────────────────────────────────────────

  function autoResize(node) {
    function resize() {
      node.style.height = 'auto'
      node.style.height = Math.min(node.scrollHeight, 120) + 'px'
    }
    node.addEventListener('input', resize)
    resize()
    return { destroy() { node.removeEventListener('input', resize) } }
  }

  // ── Context menu ───────────────────────────────────────────────────────────

  function showCtx(e, m, mine) {
    e.preventDefault()
    ctxText = m.text
    ctxMsg  = m
    ctxMine = mine
    ctxX = e.clientX
    ctxY = e.clientY
    ctxVisible = true
  }

  function copyMsg() {
    navigator.clipboard.writeText(ctxText)
    ctxVisible = false
  }

  async function saveToSaved() {
    if (!ctxMsg) return
    const text = ctxMsg.text ?? ctxMsg.file_name ?? ''
    await invoke('save_note', { text }).catch(console.error)
    ctxVisible = false
  }

  async function deleteForMe() {
    if (!ctxMsg) return
    await invoke('delete_message', { peerId, msgTs: ctxMsg.ts, forAll: false }).catch(console.error)
    removeMessage(peerId, ctxMsg.ts, ctxMsg.from)
    ctxVisible = false
  }

  async function deleteForAll() {
    if (!ctxMsg || !ctxMine) return
    await invoke('delete_message', { peerId, msgTs: ctxMsg.ts, forAll: true }).catch(console.error)
    removeMessage(peerId, ctxMsg.ts, ctxMsg.from)
    ctxVisible = false
  }

  function hideCtx() { ctxVisible = false }
</script>

<svelte:window
  on:click={() => { hideCtx(); showMuteMenu = false; showTtlMenu = false }}
  on:keydown={e => e.key === 'Escape' && (hideCtx(), showMuteMenu = false, showTtlMenu = false, cancelEdit(), showEditHistory = null, pickerVisible = {})}
/>

<div class="pane">
  <div class="header">
    {#if isSaved}
      <span class="saved-header-icon">🔖</span>
      <div class="header-info">
        <span class="peer-name">Saved Messages</span>
        <span class="peer-id">Your personal notes</span>
      </div>
    {:else if isGroup}
      <span class="group-header-icon">#</span>
      <div class="header-info">
        <span class="peer-name">
          {groupInfo?.name ?? peerId.slice(0, 8) + '…'}
          {#if groupUseSK}<span class="sk-badge" title="Sender Keys — O(1) encryption">⚡</span>{/if}
        </span>
        <span class="peer-id">{groupInfo?.members.map(m => m.username).join(', ') ?? ''}</span>
      </div>
      <button class="leave-btn" title="Leave group" on:click={leaveGroup}>Leave</button>
    {:else}
      <div class="avatar-wrap">
        <Avatar name={peerName} uid={peerId} size={32} />
        {#if $onlinePeers.has(peerId)}
          <span class="online-dot" title="Online"></span>
        {/if}
      </div>
      <div class="header-info">
        <span class="peer-name">{peerName}</span>
        <span class="peer-id">
          {peerId.slice(0, 8)}…
          {#if $onlinePeers.has(peerId)}<span class="online-label">● online</span>{/if}
        </span>
      </div>
      <button
        class="safety-btn"
        title="Verify safety number"
        on:click={() => showSafetyNumbers = true}
      >🔒</button>
    {/if}

    <!-- Mute, TTL, Export controls (hidden for Saved Messages) -->
    <div class="header-extras" class:hidden={isSaved}>
      <!-- Mute button -->
      <div class="popover-wrap">
        <button
          class="icon-hdr-btn"
          class:active-mute={muteSettings.is_muted}
          title={muteSettings.is_muted ? 'Unmute' : 'Mute'}
          on:click|stopPropagation={() => showMuteMenu = !showMuteMenu}
        >{muteSettings.is_muted ? '🔕' : '🔔'}</button>
        {#if showMuteMenu}
          <div class="popover" on:click|stopPropagation>
            {#if muteSettings.is_muted}
              <button class="pop-item" on:click={() => setMute(0)}>🔔 Unmute</button>
            {:else}
              <button class="pop-item" on:click={() => setMute(1)}>Mute 1 hour</button>
              <button class="pop-item" on:click={() => setMute(8)}>Mute 8 hours</button>
              <button class="pop-item" on:click={() => setMute(168)}>Mute 1 week</button>
              <button class="pop-item" on:click={() => setMute(null)}>Mute forever</button>
            {/if}
          </div>
        {/if}
      </div>

      <!-- TTL button -->
      <div class="popover-wrap">
        <button
          class="icon-hdr-btn"
          class:active-ttl={ttl > 0}
          title="Disappearing messages"
          on:click|stopPropagation={() => showTtlMenu = !showTtlMenu}
        >⏱</button>
        {#if showTtlMenu}
          <div class="popover" on:click|stopPropagation>
            <button class="pop-item" class:selected={ttl===0}      on:click={() => setTtl(0)}>Off</button>
            <button class="pop-item" class:selected={ttl===86400}   on:click={() => setTtl(86400)}>24 hours</button>
            <button class="pop-item" class:selected={ttl===604800}  on:click={() => setTtl(604800)}>7 days</button>
            <button class="pop-item" class:selected={ttl===2592000} on:click={() => setTtl(2592000)}>30 days</button>
          </div>
        {/if}
      </div>

      <!-- Export button -->
      <button
        class="icon-hdr-btn"
        title="Export chat"
        on:click|stopPropagation={() => showExport = !showExport}
      >⬇</button>
    </div>
  </div>

  <!-- Pinned messages banner -->
  {#if pinnedMsgs.length > 0}
    <div class="pinned-banner" on:click|stopPropagation={() => {
      if (pinnedMsgs.length === 1) scrollToPinned(pinnedMsgs[0])
      else showPinnedList = !showPinnedList
    }}>
      <span class="pin-icon">📌</span>
      <span class="pin-text">
        {#if pinnedMsgs.length === 1}
          {pinnedMsgs[0].msg_text || 'Pinned message'}
        {:else}
          {pinnedMsgs.length} pinned messages
        {/if}
      </span>
      {#if pinnedMsgs.length > 1}
        <span class="pin-chevron">{showPinnedList ? '▲' : '▼'}</span>
      {/if}
    </div>
    {#if showPinnedList}
      <div class="pinned-list">
        {#each pinnedMsgs as pin}
          <button class="pinned-list-item" on:click={() => scrollToPinned(pin)}>
            <span class="pin-item-text">{pin.msg_text || 'Attachment'}</span>
          </button>
        {/each}
      </div>
    {/if}
  {/if}

  <!-- TTL indicator banner -->
  {#if ttl > 0}
    <div class="ttl-banner">⏱ Messages disappear after {ttl >= 86400 ? ttl/86400 + 'd' : ttl/3600 + 'h'}</div>
  {/if}
  {#if expiringCount > 0}
    <div class="expiring-banner">⚠ {expiringCount} message{expiringCount > 1 ? 's' : ''} disappear in &lt;1 hour</div>
  {/if}

  <div
    class="messages-wrap"
    class:drag-over={dragging}
    on:dragover={onDragOver}
    on:dragleave={onDragLeave}
    on:drop={onDrop}
  >
  {#if dragging}
    <div class="drag-overlay">Drop files to send</div>
  {/if}
  <div class="messages" bind:this={messagesEl} on:scroll={onScroll}>
    {#if !loadedPeers.has(peerId)}
      <div class="skeleton-wrap">
        {#each [80, 55, 100, 65, 90] as w, i}
          <div class="skeleton-row" class:skeleton-mine={i % 2 === 0}>
            <div class="skeleton-bubble" style="width:{w}%"></div>
          </div>
        {/each}
      </div>
    {:else}
      {#if peerMeta[peerId]?.hasMore}
        <div class="load-more-row">
          <button class="load-more-btn" on:click={loadMore} disabled={loadingMore}>
            {loadingMore ? 'Loading…' : 'Load earlier messages'}
          </button>
        </div>
      {/if}
    {/if}

    {#each messages as m, i (m.ts + m.from)}
      {#if i === firstUnreadIdx}
        <div class="unread-divider" aria-label="New messages">
          <span>New messages</span>
        </div>
      {/if}
      {@const mine = m.from === $user?.user_id}
      {@const senderName = isGroup && !mine
        ? ($peerNames[m.from] ?? m.from?.slice(0, 8) + '…')
        : peerName}
      {@const msgKey = `${m.ts}_${m.from}`}
      {@const msgReactions = ($reactions[peerId] ?? {})[msgKey] ?? {}}
      <div class="msg" class:mine role="listitem" data-ts={m.ts} on:contextmenu={e => showCtx(e, m, mine)}>
        {#if !mine}
          <Avatar name={senderName} uid={isGroup ? m.from : peerId} size={24} />
        {/if}
        <div class="msg-col" class:mine>
          <div
            class="bubble md-content"
            class:mine
            on:mouseenter={() => showPicker(msgKey)}
            on:mouseleave={() => hidePicker(msgKey)}
          >
            {#if isGroup && !mine}
              <span class="group-sender-name">{senderName}</span>
            {/if}
            {#if m.reply_to_ts}
              <div class="reply-quote">
                <span class="reply-quote-name">{m.reply_to_from === $user?.user_id ? 'You' : senderName}</span>
                <span class="reply-quote-text">{m.reply_to_text ?? ''}</span>
              </div>
            {/if}
            {#if m.file_id || m.file_name}
              {#if m.file_mime?.startsWith('audio/')}
                <AudioPlayer msg={m} {downloadFile} {downloadingFiles} peerName={mine ? 'You' : peerName} />
              {:else if m.file_mime?.startsWith('video/')}
                <VideoPlayer msg={m} {downloadFile} {downloadingFiles} {getThumbnailUrl} {msgKey} />
              {:else if m.file_mime?.startsWith('image/')}
                {@const thumbUrl = getThumbnailUrl(msgKey, m.thumb_data)}
                {#if thumbUrl}
                  <div class="thumb-wrap">
                    <img
                      class="thumb-img"
                      src={thumbUrl}
                      alt={m.file_name ?? 'image'}
                      on:click={() => openImageLightbox(m)}
                      title="Click to open full size"
                    />
                    <div class="thumb-footer">
                      <span class="thumb-name">{m.file_name ?? 'image'}</span>
                      {#if m.file_key}
                        <button
                          class="file-dl-btn"
                          title="Download"
                          disabled={downloadingFiles[m.file_id]}
                          on:click={() => downloadFile(m)}
                        >{downloadingFiles[m.file_id] ? '…' : '⬇'}</button>
                      {/if}
                    </div>
                  </div>
                {:else}
                  <div class="file-msg">
                    <span class="file-icon">🖼</span>
                    <span class="file-info">
                      <span class="file-name">{m.file_name ?? 'image'}</span>
                      {#if m.file_size}<span class="file-size">{formatFileSize(m.file_size)}</span>{/if}
                    </span>
                    {#if m.file_key}
                      <button
                        class="file-dl-btn"
                        title="Download"
                        disabled={downloadingFiles[m.file_id]}
                        on:click={() => downloadFile(m)}
                      >{downloadingFiles[m.file_id] ? '…' : '⬇'}</button>
                    {/if}
                  </div>
                {/if}
              {:else}
                <div class="file-msg">
                  <span class="file-icon">📎</span>
                  <span class="file-info">
                    <span class="file-name">{m.file_name ?? 'file'}</span>
                    {#if m.file_size}<span class="file-size">{formatFileSize(m.file_size)}</span>{/if}
                  </span>
                  {#if m.file_key}
                    <button
                      class="file-dl-btn"
                      title="Download"
                      disabled={downloadingFiles[m.file_id]}
                      on:click={() => downloadFile(m)}
                    >{downloadingFiles[m.file_id] ? '…' : '⬇'}</button>
                  {/if}
                </div>
              {/if}
            {:else if editingMsgKey === msgKey}
              <textarea
                class="edit-input"
                bind:value={editText}
                rows="2"
                on:keydown={e => {
                  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); saveEdit(m) }
                  if (e.key === 'Escape') cancelEdit()
                }}
              />
              <div class="edit-btns">
                <button class="edit-save-btn" on:click={() => saveEdit(m)}>Save</button>
                <button class="edit-cancel-btn" on:click={cancelEdit}>Cancel</button>
              </div>
            {:else}
              {@html renderMd(m.text)}
            {/if}
            <span class="msg-meta">
              <span class="msg-ts">{formatTs(m.ts)}</span>
              {#if m.edited_at && editingMsgKey !== msgKey}
                <span
                  class="edited-label"
                  title="View edit history"
                  role="button"
                  tabindex="0"
                  on:click|stopPropagation={() => showHistory(m)}
                  on:keydown={e => e.key === 'Enter' && showHistory(m)}
                >(edited)</span>
              {/if}
              {#if mine}
                <span
                  class="msg-tick"
                  class:delivered={m.status === 'delivered'}
                  class:read={m.status === 'read'}
                >{m.status === 'sent' ? '✓' : '✓✓'}</span>
              {/if}
            </span>

            {#if pickerVisible[msgKey]}
              <div
                class="reaction-picker"
                class:mine
                class:picker-below={i < 3}
                on:mouseenter={() => keepPicker(msgKey)}
                on:mouseleave={() => hidePicker(msgKey)}
              >
                {#each QUICK_EMOJIS as emoji}
                  <button
                    class="reaction-pick-btn"
                    title={emoji}
                    on:click={() => selectReaction(m, emoji)}
                  >{emoji}</button>
                {/each}
              </div>
            {/if}
          </div>

          {#if Object.keys(msgReactions).length > 0}
            <div class="reaction-pills" class:mine>
              {#each Object.entries(msgReactions) as [emoji, reactorIds] (emoji)}
                {@const isMine = reactorIds.includes($user?.user_id ?? '')}
                <button
                  class="reaction-pill"
                  class:mine-reaction={isMine}
                  title={reactionTooltip(emoji, reactorIds)}
                  on:click={() => selectReaction(m, emoji)}
                >{emoji} {reactorIds.length}</button>
              {/each}
            </div>
          {/if}

          {#if isGroup && mine}
            {@const readers = Object.entries(groupReadMarks)
              .filter(([uid, rts]) => rts >= m.ts)
              .map(([uid]) => $peerNames[uid] ?? uid.slice(0, 6))}
            {#if readers.length > 0}
              <div class="grp-read-row" title={"Read by: " + readers.join(", ")}>
                <span class="grp-read-label">✓✓</span>
                <span class="grp-read-names">{readers.join(', ')}</span>
              </div>
            {/if}
          {/if}
        </div>

        <div class="msg-actions">
          <button
            class="reply-btn"
            title="Reply"
            on:click={() => replyTo = {
              ts: m.ts,
              from: m.from,
              peerName: mine ? 'You' : peerName,
              text: m.text,
            }}
          >↩</button>
          {#if mine && !m.file_id}
            <button
              class="edit-btn"
              title="Edit message"
              on:click={() => startEdit(m)}
            >✏</button>
          {/if}
        </div>
      </div>
    {/each}

    {#if isTyping}
      <div class="typing-row">
        <Avatar name={peerName} uid={peerId} size={20} />
        <div class="typing-bubble">
          <span class="dot"></span><span class="dot"></span><span class="dot"></span>
        </div>
      </div>
    {/if}
  </div>

  {#if showScrollBtn}
    <button class="scroll-btn" on:click={scrollBottom} title="Scroll to bottom">↓</button>
  {/if}
  </div>

  {#if replyTo}
    <div class="reply-bar">
      <div class="reply-preview">
        <span class="reply-preview-name">{replyTo.peerName}</span>
        <span class="reply-preview-text">{replyTo.text.length > 60 ? replyTo.text.slice(0, 60) + '...' : replyTo.text}</span>
      </div>
      <button class="reply-cancel" title="Cancel reply" on:click={() => replyTo = null}>✕</button>
    </div>
  {/if}

  <form class="compose" on:submit|preventDefault={send}>
    <input
      type="file"
      bind:this={fileInput}
      style="display:none"
      on:change={onFileSelected}
    />
    {#if recState === 'idle'}
    <button
      type="button"
      class="attach-btn"
      title="Attach file"
      disabled={sending}
      on:click={attachFile}
    >📎</button>
    <textarea
      bind:value={text}
      bind:this={textareaEl}
      placeholder="Message… (Enter to send, Shift+Enter for newline)"
      disabled={sending}
      rows="1"
      autocomplete="off"
      on:keydown={onKeydown}
      on:input={onInput}
      on:blur={onBlur}
      use:autoResize
    />
    {/if}
    <!-- Recording indicator (replaces textarea row while recording) -->
    {#if recState === 'voice' || recState === 'voice-locked'}
      <div class="rec-bar">
        <span class="rec-dot"></span>
        <span class="rec-time">{fmtRecTime(recSeconds)}</span>
        {#if recState === 'voice'}
          <span class="rec-hint">← swipe to cancel · ↑ lock</span>
        {:else}
          <span class="rec-hint locked-hint">🔒 locked</span>
        {/if}
      </div>
    {/if}

    <!-- Mic button: hold = voice, tap = video -->
    <button
      type="button"
      class="mic-btn"
      class:recording={recState === 'voice' || recState === 'voice-locked'}
      class:locked={recState === 'voice-locked'}
      title="Hold for voice · tap for video"
      disabled={sending}
      on:pointerdown|preventDefault={onMicPointerDown}
      on:pointermove={onMicPointerMove}
      on:pointerup={onMicPointerUp}
      on:pointercancel={() => { clearInterval(recTicker); recTicker = null; stopAndSendVoice(true) }}
    >
      {#if recState === 'voice-locked'}
        🔒
      {:else}
        <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
          <path d="M12 1a4 4 0 0 1 4 4v6a4 4 0 0 1-8 0V5a4 4 0 0 1 4-4zm6.5 9a1 1 0 0 1 1 1 7.5 7.5 0 0 1-15 0 1 1 0 1 1 2 0 5.5 5.5 0 0 0 11 0 1 1 0 0 1 1-1zM11 20.93V23a1 1 0 1 0 2 0v-2.07A8.5 8.5 0 0 0 20.47 11a1 1 0 0 0-2 0A6.5 6.5 0 0 1 12 17.5a6.5 6.5 0 0 1-6.47-6.5 1 1 0 0 0-2 0A8.5 8.5 0 0 0 11 20.93z"/>
        </svg>
      {/if}
    </button>
    {#if recState === 'voice-locked'}
      <button type="button" class="rec-stop-btn" on:click={() => { clearInterval(recTicker); recTicker = null; stopAndSendVoice(false) }}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
      </button>
    {/if}
    {#if recState === 'idle'}
      <button class="send-btn" type="submit" disabled={sending || !text.trim()} title="Send (Enter)">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
      </button>
    {/if}
  </form>

  {#if error}
    <p class="err">{error}</p>
  {/if}
</div>

{#if lightboxUrl}
  <div class="lightbox-overlay" role="dialog" aria-modal="true" on:click={() => { URL.revokeObjectURL(lightboxUrl); lightboxUrl = null }}>
    <img class="lightbox-img" src={lightboxUrl} alt="full size" on:click|stopPropagation />
    <button class="lightbox-close" on:click={() => { URL.revokeObjectURL(lightboxUrl); lightboxUrl = null }}>✕</button>
  </div>
{/if}

{#if ctxVisible}
  <div
    class="ctx-menu"
    style="left:{ctxX}px;top:{ctxY}px"
    role="menu"
    tabindex="-1"
    on:click|stopPropagation
    on:keydown|stopPropagation
  >
    <button class="ctx-item" on:click={copyMsg}>Copy</button>
    <button class="ctx-item" on:click={() => ctxMsg && togglePin(ctxMsg)}>
      {pinnedMsgs.some(p => p.msg_ts === ctxMsg?.ts && p.msg_from === ctxMsg?.from) ? 'Unpin' : 'Pin'}
    </button>
    <button class="ctx-item" on:click={saveToSaved}>Save to Saved</button>
    <div class="ctx-divider"></div>
    <button class="ctx-item ctx-delete" on:click={deleteForMe}>Delete for me</button>
    {#if ctxMine}
      <button class="ctx-item ctx-delete" on:click={deleteForAll}>Delete for all</button>
    {/if}
  </div>
{/if}

<!-- Export chat dialog -->
{#if showExport}
  <div class="modal-overlay" on:click|self={() => showExport = false}>
    <div class="modal-box">
      <div class="modal-title">Export chat</div>
      <label class="modal-label">Format
        <select bind:value={exportFormat}>
          <option value="markdown">Markdown</option>
          <option value="html">HTML</option>
          <option value="json">JSON</option>
        </select>
      </label>
      <label class="modal-check">
        <input type="checkbox" bind:checked={exportEncrypted} /> Encrypted export
      </label>
      {#if exportEncrypted}
        <input class="modal-input" type="password" bind:value={exportPassword} placeholder="Password" />
      {/if}
      <div class="modal-btns">
        <button class="modal-ok" on:click={doExport} disabled={exporting || (exportEncrypted && !exportPassword)}>
          {exporting ? '…' : 'Export'}
        </button>
        <button class="modal-cancel" on:click={() => showExport = false}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<!-- Edit history modal -->
{#if showEditHistory}
  <div class="modal-overlay" on:click|self={() => showEditHistory = null}>
    <div class="modal-box">
      <div class="modal-title">Edit history</div>
      {#if showEditHistory.history.length === 0}
        <p class="modal-empty">No history yet.</p>
      {:else}
        <ul class="hist-list">
          {#each showEditHistory.history as h}
            <li class="hist-item">
              <span class="hist-text">{h.old_plain}</span>
              <span class="hist-ts">{formatTs(h.edited_at)}</span>
            </li>
          {/each}
        </ul>
      {/if}
      <button class="modal-cancel" on:click={() => showEditHistory = null}>Close</button>
    </div>
  </div>
{/if}

{#if showSafetyNumbers && !isGroup}
  <SafetyNumbers {peerId} {peerName} on:close={() => showSafetyNumbers = false} />
{/if}

<!-- Video recording modal -->
{#if showVideoModal}
  <div class="modal-overlay video-modal-overlay" on:click|self={() => closeVideoModal(true)}>
    <div class="video-modal-box">
      <div class="video-modal-title">Video message</div>
      <!-- svelte-ignore a11y-media-has-caption -->
      <video
        class="video-preview"
        bind:this={videoPreviewEl}
        autoplay
        muted
        playsinline
      ></video>
      <div class="video-modal-btns">
        {#if !videoRecording}
          <button class="video-rec-btn" on:click={startVideoRec}>● Record</button>
        {:else}
          <button class="video-stop-btn" on:click={stopVideoRec}>■ Stop &amp; Send</button>
        {/if}
        <button class="modal-cancel" on:click={() => closeVideoModal(true)}>Cancel</button>
      </div>
      {#if videoRecording}
        <div class="video-rec-indicator">● REC</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    background: var(--bg);
  }

  .header {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .header-info { display: flex; flex-direction: column; gap: 1px; flex: 1; }
  .peer-name { font-weight: 600; font-size: 15px; }
  .peer-id   { font-size: 11px; color: var(--text-dim); font-family: monospace; }

  .safety-btn {
    background: none;
    color: var(--text-dim);
    padding: 4px 8px;
    font-size: 16px;
    border-radius: var(--radius);
    flex-shrink: 0;
  }
  .safety-btn:hover { background: var(--bg-hover); color: var(--text); }

  .saved-header-icon {
    font-size: 20px;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .group-header-icon {
    font-size: 20px;
    color: var(--text-dim);
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .sk-badge {
    font-size: 11px;
    margin-left: 4px;
    vertical-align: middle;
    opacity: 0.75;
  }

  .leave-btn {
    margin-left: auto;
    font-size: 12px;
    padding: 4px 10px;
    background: none;
    color: var(--danger);
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    cursor: pointer;
    opacity: 0.7;
    flex-shrink: 0;
  }
  .leave-btn:hover { opacity: 1; background: color-mix(in srgb, var(--danger) 12%, transparent); }
  .group-sender-name {
    display: block;
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    margin-bottom: 2px;
  }

  .messages-wrap {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .scroll-btn {
    position: absolute;
    bottom: 16px;
    right: 20px;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    font-size: 18px;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 2px 8px rgba(0,0,0,0.3);
    z-index: 10;
    border: none;
    cursor: pointer;
    padding: 0;
  }
  .scroll-btn:hover { filter: brightness(1.15); }

  /* ── Unread divider ───────────────────────────────────────────────────────── */
  .unread-divider {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 0;
    user-select: none;
  }
  .unread-divider::before,
  .unread-divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--accent);
    opacity: 0.5;
  }
  .unread-divider span {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    white-space: nowrap;
    opacity: 0.85;
  }

  .load-more-row {
    display: flex;
    justify-content: center;
    padding: 4px 0 8px;
  }
  .load-more-btn {
    background: none;
    color: var(--text-dim);
    font-size: 12px;
    padding: 4px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .load-more-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text); }
  .load-more-btn:disabled { opacity: 0.5; cursor: default; }

  .msg {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    animation: msg-in 0.18s ease;
  }
  .msg.mine { flex-direction: row-reverse; }

  .msg-col {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
    max-width: 70%;
  }
  .msg-col.mine { align-items: flex-end; }

  .bubble {
    background: var(--bg-msg-in);
    padding: 8px 12px;
    border-radius: var(--radius-msg);
    width: 100%;
    word-break: break-word;
    line-height: 1.4;
    border-bottom-left-radius: 4px;
    position: relative;
  }
  .bubble.mine {
    background: var(--bg-msg-out);
    color: #fff;
    border-bottom-left-radius: var(--radius-msg);
    border-bottom-right-radius: 4px;
  }
  :global(.bubble.mine a) { color: #bfdbfe; }

  .avatar-wrap { position: relative; flex-shrink: 0; }
  .online-dot {
    position: absolute; bottom: 0; right: 0;
    width: 8px; height: 8px;
    border-radius: 50%;
    background: var(--success);
    border: 2px solid var(--bg-panel);
  }
  .online-label { color: var(--success); margin-left: 4px; font-size: 10px; }

  .msg-meta {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 3px;
    margin-top: 2px;
  }
  .msg-ts { font-size: 10px; color: rgba(255,255,255,0.45); line-height: 1; }
  :global(.bubble:not(.mine) .msg-ts) { color: var(--text-dim); }
  .msg-tick {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.55);
    line-height: 1;
  }
  .msg-tick.delivered { color: rgba(255, 255, 255, 0.9); }
  .msg-tick.read      { color: #60a5fa; }

  /* Typing indicator */
  .typing-row {
    display: flex;
    align-items: flex-end;
    gap: 6px;
    animation: msg-in 0.18s ease;
  }
  .typing-bubble {
    background: var(--bg-msg-in);
    padding: 10px 14px;
    border-radius: var(--radius-msg);
    border-bottom-left-radius: 4px;
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-muted);
    animation: bounce 1.2s infinite ease-in-out;
  }
  .dot:nth-child(1) { animation-delay: 0s; }
  .dot:nth-child(2) { animation-delay: 0.2s; }
  .dot:nth-child(3) { animation-delay: 0.4s; }
  @keyframes bounce {
    0%, 60%, 100% { transform: translateY(0); }
    30%           { transform: translateY(-6px); }
  }

  @keyframes msg-in {
    from { opacity: 0; transform: translateY(6px); }
    to   { opacity: 1; transform: translateY(0); }
  }

  /* ── Reply quote (inside bubble) ─────────────────────────────────────── */
  .reply-quote {
    background: rgba(0,0,0,0.12);
    border-left: 3px solid var(--accent);
    padding: 4px 8px;
    margin-bottom: 6px;
    border-radius: 2px;
    font-size: 12px;
    overflow: hidden;
  }
  .reply-quote-name {
    font-weight: 600;
    display: block;
    margin-bottom: 1px;
    color: var(--accent);
  }
  .reply-quote-text {
    display: block;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    opacity: 0.8;
  }

  /* ── Reply button (shown on hover) ──────────────────────────────────── */
  .reply-btn {
    background: none;
    opacity: 0;
    transition: opacity 0.15s;
    color: var(--text-dim);
    font-size: 14px;
    padding: 2px 6px;
    border-radius: var(--radius);
    flex-shrink: 0;
    align-self: flex-end;
    margin-bottom: 4px;
  }
  .msg:hover .reply-btn { opacity: 1; }
  .reply-btn:hover { background: var(--bg-hover); color: var(--text); }

  /* ── Reply bar (above compose) ───────────────────────────────────────── */
  .reply-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 16px;
    border-top: 1px solid var(--border);
    background: var(--bg-panel);
  }
  .reply-preview {
    flex: 1;
    min-width: 0;
    border-left: 3px solid var(--accent);
    padding-left: 8px;
  }
  .reply-preview-name {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    display: block;
  }
  .reply-preview-text {
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    display: block;
  }
  .reply-cancel {
    background: none;
    color: var(--text-dim);
    font-size: 14px;
    padding: 2px 6px;
    border-radius: var(--radius);
    flex-shrink: 0;
  }
  .reply-cancel:hover { background: var(--bg-hover); color: var(--text); }

  /* ── File message ──────────────────────────────────────────────────────── */
  .file-msg {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 0 4px;
  }
  .file-icon { font-size: 18px; flex-shrink: 0; }
  .file-info { display: flex; flex-direction: column; flex: 1; min-width: 0; }
  .file-name {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .file-size { font-size: 11px; opacity: 0.7; }
  .file-dl-btn {
    background: none;
    color: inherit;
    font-size: 16px;
    padding: 2px 6px;
    border-radius: var(--radius);
    flex-shrink: 0;
    opacity: 0.85;
  }
  .file-dl-btn:hover:not(:disabled) { background: rgba(0,0,0,0.12); opacity: 1; }
  .file-dl-btn:disabled { cursor: default; }

  /* ── Inline thumbnail ─────────────────────────────────────────────────── */
  .thumb-wrap {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-width: 220px;
    padding: 2px 0 4px;
  }
  .thumb-img {
    max-width: 200px;
    max-height: 200px;
    border-radius: calc(var(--radius) - 2px);
    cursor: pointer;
    display: block;
    object-fit: contain;
    background: rgba(0,0,0,0.08);
  }
  .thumb-img:hover { opacity: 0.88; }
  .thumb-footer {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .thumb-name {
    font-size: 11px;
    opacity: 0.7;
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* ── Lightbox ──────────────────────────────────────────────────────────── */
  .lightbox-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.85);
    z-index: 999;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: zoom-out;
  }
  .lightbox-img {
    max-width: 90vw;
    max-height: 90vh;
    object-fit: contain;
    border-radius: var(--radius);
    cursor: default;
    box-shadow: 0 8px 40px rgba(0,0,0,0.6);
  }
  .lightbox-close {
    position: absolute;
    top: 16px;
    right: 16px;
    background: rgba(255,255,255,0.15);
    color: #fff;
    font-size: 18px;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .lightbox-close:hover { background: rgba(255,255,255,0.28); }

  /* ── Attach button ─────────────────────────────────────────────────────── */
  .attach-btn {
    background: none;
    color: var(--text-dim);
    font-size: 16px;
    padding: 0 6px;
    height: 36px;
    border-radius: var(--radius);
    flex-shrink: 0;
    align-self: flex-end;
  }
  .attach-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text); }

  .compose {
    display: flex;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border);
    background: var(--bg-panel);
    align-items: flex-end;
  }
  .compose textarea {
    flex: 1;
    min-height: 36px;
    max-height: 120px;
    line-height: 1.4;
  }
  .compose button { height: 36px; align-self: flex-end; }

  .err { padding: 4px 16px 8px; color: var(--danger); font-size: 12px; }

  /* ── Reactions ─────────────────────────────────────────────────────────────── */
  .reaction-picker {
    position: absolute;
    bottom: calc(100% + 4px);
    left: 0;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 4px 8px;
    display: flex;
    gap: 2px;
    box-shadow: 0 4px 12px rgba(0,0,0,0.3);
    z-index: 20;
    white-space: nowrap;
  }
  .reaction-picker.picker-below {
    bottom: auto;
    top: calc(100% + 4px);
  }
  .reaction-picker.mine { left: auto; right: 0; }
  .reaction-pick-btn {
    background: none;
    font-size: 18px;
    padding: 2px 3px;
    border-radius: 50%;
    line-height: 1;
    transition: transform 0.1s;
  }
  .reaction-pick-btn:hover { transform: scale(1.3); background: var(--bg-hover); }

  .reaction-pills {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 0 2px;
  }
  .reaction-pill {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 13px;
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    color: var(--text);
    cursor: pointer;
    animation: reaction-appear 0.2s ease-out;
    transition: background 0.12s;
  }
  .reaction-pill:hover { background: var(--bg-hover); }
  .reaction-pill.mine-reaction {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    border-color: var(--accent);
  }
  @keyframes reaction-appear {
    from { opacity: 0; transform: scale(0.8); }
    to   { opacity: 1; transform: scale(1); }
  }

  /* Context menu */
  .ctx-menu {
    position: fixed;
    z-index: 200;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 4px 12px rgba(0,0,0,0.4);
    overflow: hidden;
    min-width: 120px;
  }
  .ctx-item {
    display: block;
    width: 100%;
    background: none;
    color: var(--text);
    text-align: left;
    padding: 8px 14px;
    font-size: 13px;
    border-radius: 0;
  }
  .ctx-item:hover { background: var(--bg-hover); }
  .ctx-delete { color: #e05555; }
  .ctx-delete:hover { background: rgba(224,85,85,0.12); }

  /* ── Group read receipts ─────────────────────────────────────────────────── */
  .grp-read-row {
    display: flex; align-items: center; gap: 3px;
    margin-top: 2px; cursor: default;
  }
  .grp-read-label { font-size: 11px; color: #4caf7d; }
  .grp-read-names { font-size: 10px; color: var(--text-muted); }

  /* ── Header extras ───────────────────────────────────────────────────────── */
  .header-extras { display: flex; align-items: center; gap: 4px; margin-left: auto; flex-shrink: 0; }
  .header-extras.hidden { display: none; }
  .icon-hdr-btn {
    background: none; border: none; cursor: pointer;
    color: var(--text-muted); font-size: 14px; padding: 3px 5px; border-radius: 4px;
  }
  .icon-hdr-btn:hover { background: var(--bg-hover); color: var(--text); }
  .icon-hdr-btn.active-mute { color: var(--danger, #f38ba8); }
  .icon-hdr-btn.active-ttl  { color: var(--accent); }

  /* ── Popovers ─────────────────────────────────────────────────────────────── */
  .popover-wrap { position: relative; }
  .popover {
    position: absolute; top: calc(100% + 4px); right: 0; z-index: 50;
    background: var(--bg-2, #1e1e2e); border: 1px solid var(--border, #313244);
    border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    min-width: 140px; padding: 4px;
  }
  .pop-item {
    display: block; width: 100%; background: none; color: var(--text);
    text-align: left; padding: 7px 12px; font-size: 12px; border-radius: 4px; cursor: pointer;
  }
  .pop-item:hover { background: var(--bg-hover); }
  .pop-item.selected { color: var(--accent); font-weight: 600; }

  /* ── TTL banners ──────────────────────────────────────────────────────────── */
  .ttl-banner {
    padding: 5px 16px; font-size: 11px; text-align: center;
    background: rgba(137,180,250,0.08); color: var(--accent);
  }
  .expiring-banner {
    padding: 5px 16px; font-size: 11px; text-align: center;
    background: rgba(243,139,168,0.1); color: #f38ba8;
  }

  /* ── Edit mode ────────────────────────────────────────────────────────────── */
  .edit-input {
    width: 100%; box-sizing: border-box; resize: none;
    background: var(--bg-input, #313244); color: var(--text);
    border: 1px solid var(--accent); border-radius: 4px;
    padding: 6px 8px; font-size: 13px; font-family: inherit; line-height: 1.4;
  }
  .edit-btns { display: flex; gap: 6px; margin-top: 4px; }
  .edit-save-btn {
    font-size: 11px; padding: 3px 10px; border-radius: 4px;
    background: var(--accent, #89b4fa); color: #1e1e2e; border: none; cursor: pointer;
  }
  .edit-cancel-btn {
    font-size: 11px; padding: 3px 8px; border-radius: 4px;
    background: none; color: var(--text-muted); border: 1px solid var(--border); cursor: pointer;
  }
  .edited-label {
    font-size: 10px; color: var(--text-dim, #6c7086);
    cursor: pointer; margin-left: 4px; vertical-align: middle;
  }
  .edited-label:hover { text-decoration: underline; }

  /* ── Msg-actions row (reply + edit buttons) ───────────────────────────────── */
  .msg-actions { display: flex; flex-direction: column; gap: 2px; align-self: center; flex-shrink: 0; }
  .edit-btn {
    background: none; border: none; cursor: pointer; color: var(--text-dim);
    font-size: 12px; padding: 2px 4px; border-radius: 3px; opacity: 0;
  }
  .msg:hover .edit-btn { opacity: 1; }
  .edit-btn:hover { background: var(--bg-hover); color: var(--text); opacity: 1 !important; }

  /* ── Modal shared ─────────────────────────────────────────────────────────── */
  .modal-overlay {
    position: fixed; inset: 0; background: rgba(0,0,0,0.5);
    display: flex; align-items: center; justify-content: center; z-index: 200;
  }
  .modal-box {
    background: var(--bg-2, #1e1e2e); border: 1px solid var(--border, #313244);
    border-radius: 12px; padding: 20px; min-width: 280px; max-width: 380px;
    box-shadow: 0 16px 48px rgba(0,0,0,0.5);
  }
  .modal-title { font-size: 14px; font-weight: 600; color: var(--text); margin-bottom: 14px; }
  .modal-label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-muted); margin-bottom: 10px; }
  .modal-label select { padding: 5px 8px; font-size: 12px; background: var(--bg-input, #313244); color: var(--text); border: 1px solid var(--border); border-radius: 4px; }
  .modal-check { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--text-muted); margin-bottom: 10px; }
  .modal-input { width: 100%; box-sizing: border-box; padding: 6px 8px; font-size: 12px; background: var(--bg-input, #313244); color: var(--text); border: 1px solid var(--border); border-radius: 4px; margin-bottom: 10px; }
  .modal-btns { display: flex; gap: 8px; margin-top: 4px; }
  .modal-ok { flex: 1; padding: 7px; font-size: 12px; background: var(--accent, #89b4fa); color: #1e1e2e; border: none; border-radius: 6px; cursor: pointer; }
  .modal-ok:disabled { opacity: 0.5; cursor: default; }
  .modal-cancel { flex: 1; padding: 7px; font-size: 12px; background: none; color: var(--text-muted); border: 1px solid var(--border); border-radius: 6px; cursor: pointer; }
  .modal-empty { font-size: 12px; color: var(--text-dim); text-align: center; padding: 12px 0; }

  /* ── Edit history list ────────────────────────────────────────────────────── */
  .hist-list { list-style: none; margin: 0 0 12px; padding: 0; max-height: 240px; overflow-y: auto; }
  .hist-item { padding: 8px; border-bottom: 1px solid var(--border); }
  .hist-text { display: block; font-size: 12px; color: var(--text); margin-bottom: 2px; }
  .hist-ts { font-size: 10px; color: var(--text-dim); }

  /* ── Mic button ───────────────────────────────────────────────────────────── */
  .mic-btn {
    background: none;
    color: var(--text-dim);
    font-size: 16px;
    padding: 0 6px;
    height: 36px;
    border-radius: var(--radius);
    flex-shrink: 0;
    align-self: flex-end;
    user-select: none;
    touch-action: none;
    transition: color 0.15s, background 0.15s;
  }
  .mic-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text); }
  .mic-btn.recording {
    color: #f38ba8;
    animation: mic-pulse 1s infinite;
  }
  .mic-btn.locked { color: #a6e3a1; animation: none; }
  @keyframes mic-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.45; }
  }

  .rec-stop-btn {
    height: 36px;
    align-self: flex-end;
    padding: 0 14px;
    background: #f38ba8;
    color: #1e1e2e;
    border: none;
    border-radius: var(--radius);
    font-size: 13px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .rec-stop-btn:hover { filter: brightness(1.1); }

  /* ── Video modal ─────────────────────────────────────────────────────────── */
  .video-modal-overlay { z-index: 300; }
  .video-modal-box {
    background: var(--bg-2, #1e1e2e);
    border: 1px solid var(--border, #313244);
    border-radius: 14px;
    padding: 16px;
    width: 420px;
    max-width: 95vw;
    box-shadow: 0 16px 48px rgba(0,0,0,0.6);
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .video-modal-title { font-size: 14px; font-weight: 600; color: var(--text); }
  .video-preview {
    width: 100%;
    border-radius: 8px;
    background: #000;
    aspect-ratio: 4/3;
    object-fit: cover;
  }
  .video-modal-btns { display: flex; gap: 8px; }
  .video-rec-btn {
    flex: 1; padding: 8px; font-size: 13px;
    background: #f38ba8; color: #1e1e2e;
    border: none; border-radius: 6px; cursor: pointer; font-weight: 600;
  }
  .video-rec-btn:hover { filter: brightness(1.1); }
  .video-stop-btn {
    flex: 1; padding: 8px; font-size: 13px;
    background: var(--accent, #89b4fa); color: #1e1e2e;
    border: none; border-radius: 6px; cursor: pointer; font-weight: 600;
  }
  .video-stop-btn:hover { filter: brightness(1.1); }
  .video-rec-indicator {
    position: absolute; top: 16px; right: 16px;
    font-size: 11px; font-weight: 700; color: #f38ba8;
    animation: mic-pulse 1s infinite;
  }

  /* ── Recording bar (inline, replaces textarea hint) ─────────────────────── */
  .rec-bar {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 4px;
    min-height: 36px;
    align-self: flex-end;
  }
  .rec-dot {
    width: 8px; height: 8px; border-radius: 50%;
    background: #f38ba8; flex-shrink: 0;
    animation: mic-pulse 1s infinite;
  }
  .rec-time {
    font-size: 15px; font-weight: 600; font-variant-numeric: tabular-nums;
    color: var(--text); min-width: 36px;
  }
  .rec-hint {
    font-size: 11px; color: var(--text-dim); flex: 1;
  }
  .locked-hint { color: #a6e3a1; }

  /* ── Pinned messages ─────────────────────────────────────────────────────── */
  .pinned-banner {
    display: flex; align-items: center; gap: 8px;
    padding: 6px 14px; cursor: pointer;
    background: var(--bg-hover); border-bottom: 1px solid var(--border);
    font-size: 13px; color: var(--text);
    transition: background 0.12s;
  }
  .pinned-banner:hover { background: var(--bg-active); }
  .pin-icon { flex-shrink: 0; }
  .pin-text { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; opacity: 0.85; }
  .pin-chevron { font-size: 10px; opacity: 0.6; }
  .pinned-list {
    background: var(--bg-panel); border-bottom: 1px solid var(--border);
    max-height: 160px; overflow-y: auto;
  }
  .pinned-list-item {
    display: block; width: 100%; text-align: left;
    padding: 7px 14px 7px 34px; font-size: 12px;
    background: none; color: var(--text);
    border-bottom: 1px solid var(--border-sub);
  }
  .pinned-list-item:hover { background: var(--bg-hover); }
  .pin-item-text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; display: block; }

  .ctx-divider { height: 1px; background: var(--border); margin: 3px 0; }

  /* ── Send button ─────────────────────────────────────────────────────────── */
  .send-btn {
    width: 36px; height: 36px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    display: flex; align-items: center; justify-content: center;
    flex-shrink: 0;
    transition: background 0.15s, transform 0.1s;
  }
  .send-btn:hover:not(:disabled) { background: var(--accent-dim, color-mix(in srgb, var(--accent) 80%, #fff)); }
  .send-btn:active:not(:disabled) { transform: scale(0.93); }
  .send-btn:disabled { background: var(--bg-hover); color: var(--text-dim); }

  /* ── Mic button (accent color) ───────────────────────────────────────────── */
  .mic-btn {
    color: var(--accent);
  }
  .mic-btn.recording { color: #f38ba8; }

  /* ── Drag & drop overlay ─────────────────────────────────────────────────── */
  .messages-wrap { position: relative; }
  .drag-over { outline: 2px dashed var(--accent); outline-offset: -4px; }
  .drag-overlay {
    position: absolute; inset: 0; z-index: 10;
    display: flex; align-items: center; justify-content: center;
    background: rgba(0,0,0,0.45);
    font-size: 18px; font-weight: 600; color: #fff;
    border-radius: 6px;
    pointer-events: none;
  }

  /* ── Skeleton loader ─────────────────────────────────────────────────────── */
  .skeleton-wrap { padding: 16px 12px; display: flex; flex-direction: column; gap: 10px; }
  .skeleton-row { display: flex; }
  .skeleton-mine { justify-content: flex-end; }
  .skeleton-bubble {
    max-width: 60%;
    height: 36px;
    border-radius: 12px;
    background: linear-gradient(90deg, var(--bg-hover) 25%, var(--bg-panel) 50%, var(--bg-hover) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.4s infinite;
  }
  @keyframes shimmer { from { background-position: 200% 0 } to { background-position: -200% 0 } }

  /* ── Syntax highlighting (hljs) ─────────────────────────────────────────── */
  :global(pre) {
    margin: 6px 0;
    border-radius: 6px;
    overflow-x: auto;
  }
  :global(pre code.hljs) {
    display: block;
    padding: 12px 14px;
    font-size: 12px;
    line-height: 1.5;
    font-family: 'Cascadia Code', 'Fira Code', 'Consolas', monospace;
    border-radius: 6px;
  }
  /* Dark theme tokens */
  :global(.hljs) { background: #1e1e2e; color: #cdd6f4; }
  :global(.hljs-keyword)    { color: #cba6f7; }
  :global(.hljs-built_in)   { color: #89dceb; }
  :global(.hljs-type)       { color: #f9e2af; }
  :global(.hljs-literal)    { color: #fab387; }
  :global(.hljs-number)     { color: #fab387; }
  :global(.hljs-string)     { color: #a6e3a1; }
  :global(.hljs-comment)    { color: #6c7086; font-style: italic; }
  :global(.hljs-function)   { color: #89b4fa; }
  :global(.hljs-title)      { color: #89b4fa; }
  :global(.hljs-attr)       { color: #89dceb; }
  :global(.hljs-variable)   { color: #cdd6f4; }
  :global(.hljs-tag)        { color: #f38ba8; }
  :global(.hljs-name)       { color: #cba6f7; }
  :global(.hljs-operator)   { color: #89dceb; }
  :global(.hljs-punctuation){ color: #cdd6f4; }
  :global(.hljs-meta)       { color: #f9e2af; }
  :global(.hljs-section)    { color: #89b4fa; font-weight: bold; }
  :global(.hljs-selector-class) { color: #fab387; }
  :global(.hljs-selector-id)    { color: #f38ba8; }
  :global(.hljs-addition)   { color: #a6e3a1; }
  :global(.hljs-deletion)   { color: #f38ba8; }
  /* Light theme overrides */
  :global([data-theme="light"] .hljs) { background: #f0f0f0; color: #24292e; }
  :global([data-theme="light"] .hljs-keyword)    { color: #d73a49; }
  :global([data-theme="light"] .hljs-built_in)   { color: #005cc5; }
  :global([data-theme="light"] .hljs-string)     { color: #032f62; }
  :global([data-theme="light"] .hljs-number)     { color: #e36209; }
  :global([data-theme="light"] .hljs-comment)    { color: #6a737d; }
  :global([data-theme="light"] .hljs-function)   { color: #6f42c1; }
  :global([data-theme="light"] .hljs-title)      { color: #6f42c1; }
  :global([data-theme="light"] .hljs-tag)        { color: #22863a; }
  :global([data-theme="light"] .hljs-attr)       { color: #005cc5; }
</style>
