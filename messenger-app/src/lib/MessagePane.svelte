<script>
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { tick, onMount, onDestroy } from 'svelte'
  import { marked } from 'marked'
  import DOMPurify from 'dompurify'
  import hljs from 'highlight.js'
  import { conversations, activeConv, user, peerNames, typingPeers, addMessage, addGroupMessage, removeMessage, onlinePeers, unlocked, groups, unreadCounts, reactions, showSearch, channels, chatBg, CHAT_GRADIENTS } from '../stores.js'
  import Avatar from './Avatar.svelte'
  import SafetyNumbers from './SafetyNumbers.svelte'
  import AudioPlayer from './AudioPlayer.svelte'
  import VideoPlayer from './VideoPlayer.svelte'
  import PollView from './PollView.svelte'
  import SlashCommandPalette from './SlashCommandPalette.svelte'
  import StickerPicker from './StickerPicker.svelte'
  import GroupEventModal from './GroupEventModal.svelte'
  import LinkPreviewCard from './LinkPreviewCard.svelte'
  import ThreadPane from './ThreadPane.svelte'

  export let peerId
  export let onToggleSidebar = null
  export let onToggleDetail = null
  export let detailOpen = false

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
    openThread = null
    invoke('get_draft', { peerId }).then(d => { text = d }).catch(() => {})
  }

  // ── Mute settings ──────────────────────────────────────────────────────────

  let muteSettings = { notifications_enabled: true, mute_until: 0, is_muted: false }
  let showMoreMenu = false

  $: if (peerId && $unlocked) {
    invoke('get_mute', { peerId }).then(s => { muteSettings = s }).catch(() => {})
  }

  async function setMute(hours) {
    showMoreMenu = false
    await invoke('set_mute', { peerId, muteHours: hours }).catch(() => {})
    muteSettings = await invoke('get_mute', { peerId }).catch(() => muteSettings)
  }

  // ── TTL (disappearing messages) ────────────────────────────────────────────

  let ttl = 0
  let expiringCount = 0
  let expiryCheckTimer = null

  $: if (peerId && $unlocked) {
    invoke('get_ttl', { peerId }).then(t => { ttl = t }).catch(() => {})
  }

  async function setTtl(secs) {
    showMoreMenu = false
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

  // ── Polls ──────────────────────────────────────────────────────────────────

  let showAttachMenu = false
  let showPollModal = false
  let pollQuestion = ''
  let pollOptions = ['', '']
  let creatingPoll = false

  function parsePollId(text) {
    const m = text?.match(/^\[poll:([0-9a-f-]{36})\]$/)
    return m ? m[1] : null
  }

  function addPollOption() {
    if (pollOptions.length < 10) pollOptions = [...pollOptions, '']
  }
  function removePollOption(i) {
    if (pollOptions.length > 2) pollOptions = pollOptions.filter((_, idx) => idx !== i)
  }

  async function createAndSendPoll() {
    const q = pollQuestion.trim()
    const opts = pollOptions.map(o => o.trim()).filter(Boolean)
    if (!q || opts.length < 2) return
    creatingPoll = true
    try {
      const pollId = await invoke('create_poll', { peerId, question: q, options: opts })
      const markerText = `[poll:${pollId}]`
      if (isGroup) {
        await invoke('send_group_message', { groupId: peerId, text: markerText })
        addGroupMessage(peerId, {
          from: $user.user_id, text: markerText, ts: Date.now(), status: 'sent',
          group_id: peerId, sender_id: $user.user_id,
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null, thumb_data: null,
        })
      } else {
        const { id } = await invoke('send_message', { peerId, text: markerText })
        addMessage(peerId, {
          from: $user.user_id, text: markerText, ts: Date.now(), id, status: 'sent',
          reply_to_ts: null, reply_to_from: null, reply_to_text: null,
          file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null, thumb_data: null,
        })
      }
      showPollModal = false
      pollQuestion = ''
      pollOptions = ['', '']
      await tick(); scrollBottom()
    } catch (e) { error = String(e) }
    creatingPoll = false
  }

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

  // Thread pane: { ts, from, text } of the parent message, or null when closed
  let openThread = null

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
    const _isGrp = !isChannel && !!$groups[peerId]
    const _convKey = convKey
    const _chanId = channelId
    // Fetch last-read timestamp in parallel with history load
    invoke('get_last_read_ts', { peerId: _convKey }).then(ts => {
      lastReadByPeer = { ...lastReadByPeer, [_convKey]: ts }
    }).catch(() => {})
    // Load per-member read marks for group chats (not channels)
    if (_isGrp) {
      invoke('get_group_read_marks', { groupId: peerId })
        .then(marks => { groupReadMarks = marks })
        .catch(() => {})
    }
    const loadPromise = isChannel
      ? invoke('get_group_messages', { groupId: _chanId, limit: PAGE_SIZE, beforeId: null })
      : (_isGrp
        ? invoke('get_group_messages', { groupId: peerId, limit: PAGE_SIZE, beforeId: null })
        : invoke('get_messages', { peerId, limit: PAGE_SIZE, beforeId: null }))
    loadPromise.then(history => {
      conversations.update(c => ({ ...c, [_convKey]: history }))
      peerMeta[peerId] = {
        oldestDbId: history.length > 0 ? history[0].db_id : null,
        hasMore: history.length === PAGE_SIZE,
      }
      if (!_isGrp && !isChannel && history.some(m => m.from === peerId)) {
        invoke('send_read_receipt', { peerId }).catch(() => {})
      }
    }).catch(() => {})
    // Load pending scheduled messages for this conversation.
    invoke('list_scheduled', { peerId: _convKey }).then(l => { scheduledMsgs = l }).catch(() => {})
    // Load retention setting.
    invoke('get_retention', { peerId: _convKey }).then(c => { retentionCount = c }).catch(() => {})
  }

  // When switching to a conversation, schedule mark-as-read after 1 s
  $: if (peerId) {
    clearTimeout(readTimers[peerId])
    const _ck = convKey
    const _isGrp2 = isGroup
    readTimers[peerId] = setTimeout(() => {
      const msgs = $conversations[_ck] ?? []
      const maxTs = msgs.reduce((acc, m) => m.ts > acc ? m.ts : acc, 0)
      if (maxTs > 0) {
        invoke('mark_as_read', { peerId: _ck, ts: maxTs }).catch(() => {})
        lastReadByPeer = { ...lastReadByPeer, [_ck]: maxTs }
        unreadCounts.update(c => { const n = { ...c }; delete n[_ck]; return n })
        if (_isGrp2) {
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
    const msgs = $conversations[convKey] ?? []
    const lr = lastReadByPeer[convKey] ?? 0
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
      const more = await (isChannel
        ? invoke('get_group_messages', { groupId: channelId, limit: PAGE_SIZE, beforeId: meta.oldestDbId })
        : isGroup
        ? invoke('get_group_messages', { groupId: peerId, limit: PAGE_SIZE, beforeId: meta.oldestDbId })
        : invoke('get_messages', { peerId, limit: PAGE_SIZE, beforeId: meta.oldestDbId }))
      if (more.length > 0) {
        peerMeta[peerId] = { oldestDbId: more[0].db_id, hasMore: more.length === PAGE_SIZE }
        conversations.update(c => ({ ...c, [convKey]: [...more, ...(c[convKey] ?? [])] }))
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
  }

  function scrollToPinned(pin) {
    const el = messagesEl?.querySelector(`[data-ts="${pin.msg_ts}"]`)
    if (el) { el.scrollIntoView({ behavior: 'smooth', block: 'center' }); showPinnedList = false }
  }

  const renderer = new marked.Renderer()
  renderer.code = ({ text, lang }) => {
    const language = lang && hljs.getLanguage(lang) ? lang : 'plaintext'
    const highlighted = hljs.highlight(text, { language }).value
    return `<pre><code class="hljs language-${language}">${highlighted}</code></pre>`
  }
  marked.setOptions({ breaks: true, gfm: true, renderer })

  const PURIFY_OPTS = {
    ALLOWED_TAGS: ['strong','em','del','code','pre','br','p','ul','ol','li','blockquote','a','h1','h2','h3','h4','span','mark'],
    ALLOWED_ATTR: ['class','href'],
  }

  function renderMd(raw) {
    return DOMPurify.sanitize(marked.parse(raw ?? ''), PURIFY_OPTS)
  }

  $: isSaved = peerId === '__saved__'
  $: isChannel = typeof peerId === 'string' && peerId.includes('/') && !isSaved
  $: channelGroupId = isChannel ? peerId.split('/')[0] : null
  $: channelId = isChannel ? peerId.split('/')[1] : null
  $: channelInfo = isChannel ? ($channels[channelGroupId] ?? []).find(c => c.channel_id === channelId) : null
  $: channelGroupInfo = isChannel ? $groups[channelGroupId] : null
  $: convKey = isChannel ? channelId : peerId

  $: messages = $conversations[convKey] ?? []
  $: peerName = $peerNames[peerId] ?? peerId?.slice(0, 8) + '…'
  $: isTyping = !!$typingPeers[peerId]
  $: isGroup = !isChannel && !!$groups[peerId]
  $: groupInfo = $groups[peerId] ?? null

  // ── Sticker picker ────────────────────────────────────────────────────────────
  let showStickerPicker = false

  // ── Event modal ───────────────────────────────────────────────────────────────
  let showEventModal = false

  // ── C6 Message Scheduling ─────────────────────────────────────────────────────
  let showScheduleModal = false
  let scheduleDatetime = ''
  let scheduledMsgs = []
  let showScheduledList = false

  // ── C15 Data Retention ────────────────────────────────────────────────────────
  let retentionCount = 0
  const RETENTION_OPTIONS = [
    { label: 'Unlimited', value: 0 },
    { label: 'Last 500', value: 500 },
    { label: 'Last 1,000', value: 1000 },
    { label: 'Last 2,000', value: 2000 },
    { label: 'Last 5,000', value: 5000 },
  ]

  async function setRetention(count) {
    retentionCount = count
    showMoreMenu = false
    try {
      await invoke('set_retention', { peerId: convKey, retentionCount: count })
      if (count > 0) {
        const q = isChannel
          ? invoke('get_group_messages', { groupId: channelId, limit: PAGE_SIZE, beforeId: null })
          : isGroup
            ? invoke('get_group_messages', { groupId: peerId, limit: PAGE_SIZE, beforeId: null })
            : invoke('get_messages', { peerId, limit: PAGE_SIZE, beforeId: null })
        const updated = await q.catch(() => null)
        if (updated) conversations.update(c => ({ ...c, [convKey]: updated }))
      }
    } catch (e) { error = String(e) }
  }

  // ── C3 Link Preview ───────────────────────────────────────────────────────────
  const URL_REGEX = /https?:\/\/[^\s<>"']+/g
  let linkPreviews = {}       // msgKey → LinkPreview | null | 'loading'
  let previewQueue = []
  let isFetchingPreviews = false

  function extractFirstUrl(text) {
    const m = text?.match(URL_REGEX)
    return m ? m[0] : null
  }

  async function processPreviewQueue() {
    if (isFetchingPreviews) return
    isFetchingPreviews = true
    while (previewQueue.length > 0) {
      const { msgKey, url } = previewQueue.shift()
      if (linkPreviews[msgKey] !== undefined && linkPreviews[msgKey] !== 'loading') continue
      const p = await invoke('fetch_link_preview', { url }).catch(() => null)
      linkPreviews = { ...linkPreviews, [msgKey]: p ?? null }
      if (previewQueue.length > 0) await new Promise(r => setTimeout(r, 80))
    }
    isFetchingPreviews = false
  }

  $: {
    for (const m of messages) {
      if (!m.sticker && !m.location && !m.event_data && m.text) {
        const key = `${m.ts}_${m.from}`
        if (linkPreviews[key] === undefined) {
          const url = extractFirstUrl(m.text)
          if (url) {
            linkPreviews[key] = 'loading'
            previewQueue.push({ msgKey: key, url })
            processPreviewQueue()
          } else {
            linkPreviews[key] = null
          }
        }
      }
    }
  }

  // ── Translations ──────────────────────────────────────────────────────────────
  let translations = {}
  let translating = {}

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

  async function sendFile(file) {
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

  async function onFileSelected(e) {
    const file = e.target.files?.[0]
    if (!file) return
    e.target.value = ''
    await sendFile(file)
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
      } else if (isChannel) {
        const mentionIds = mentions.map(m => m.id)
        await invoke('send_channel_message', { channelId, groupId: channelGroupId, text: msg, replyTo: replyArg, mentions: mentionIds, payloadOverride: null })
        addGroupMessage(channelId, {
          from: $user.user_id,
          text: msg,
          ts: Date.now(),
          status: 'sent',
          group_id: channelId,
          sender_id: $user.user_id,
          reply_to_ts:   replyTo?.ts   ?? null,
          reply_to_from: replyTo?.from ?? null,
          reply_to_text: replyTo?.text ?? null,
          file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null,
          mentions: mentionIds,
        })
      } else if (isGroup) {
        const mentionIds = mentions.map(m => m.id)
        await invoke('send_group_message', { groupId: peerId, text: msg, replyTo: replyArg, mentions: mentionIds })
        addGroupMessage(peerId, {
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
          mentions: mentionIds,
        })
      } else {
        const mentionIds = mentions.map(m => m.id)
        const { id, ts: msgTs } = await invoke('send_message', { peerId, text: msg, replyTo: replyArg, mentions: mentionIds })
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
          mentions: mentionIds,
        })
      }
      text = ''
      replyTo = null
      mentions = []
      saveDraft(peerId, '')
      await tick()
      scrollBottom()
    } catch (e) {
      error = String(e)
    } finally {
      sending = false
      await tick()
      textareaEl?.focus()
    }
  }

  // ── Sticker send ──────────────────────────────────────────────────────────────

  async function sendSticker(sticker) {
    showStickerPicker = false
    const payload = JSON.stringify({ sticker: { pack: sticker.pack, id: sticker.id } })
    const ts = Date.now()
    try {
      if (isChannel) {
        await invoke('send_channel_message', { channelId, groupId: channelGroupId, text: '', payloadOverride: payload })
        addGroupMessage(channelId, { from: $user.user_id, text: '', ts, status: 'sent', group_id: channelId, sender_id: $user.user_id, sticker: sticker, reply_to_ts: null, reply_to_from: null, reply_to_text: null, file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null })
      } else if (isGroup) {
        await invoke('send_group_message', { groupId: peerId, text: '', payloadOverride: payload })
        addGroupMessage(peerId, { from: $user.user_id, text: '', ts, status: 'sent', group_id: peerId, sender_id: $user.user_id, sticker: sticker, reply_to_ts: null, reply_to_from: null, reply_to_text: null, file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null })
      }
      await tick(); scrollBottom()
    } catch (e) { error = String(e) }
  }

  // ── Location send ─────────────────────────────────────────────────────────────

  async function sendLocation() {
    if (!navigator.geolocation) { error = 'Geolocation not available'; return }
    let pos
    try {
      pos = await new Promise((res, rej) => navigator.geolocation.getCurrentPosition(res, rej, { timeout: 10_000 }))
    } catch (e) {
      error = 'Location denied or unavailable'; return
    }
    const { latitude: lat, longitude: lng, accuracy: acc } = pos.coords
    if (!confirm(`Share your location? (±${Math.round(acc)}m accuracy)`)) return
    const payload = JSON.stringify({ location: { lat, lng, acc: Math.round(acc) } })
    const ts = Date.now()
    const locObj = { lat, lng, acc: Math.round(acc) }
    try {
      if (isChannel) {
        await invoke('send_channel_message', { channelId, groupId: channelGroupId, text: '', payloadOverride: payload })
        addGroupMessage(channelId, { from: $user.user_id, text: '', ts, status: 'sent', group_id: channelId, sender_id: $user.user_id, location: locObj, reply_to_ts: null, reply_to_from: null, reply_to_text: null, file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null })
      } else if (isGroup) {
        await invoke('send_group_message', { groupId: peerId, text: '', payloadOverride: payload })
        addGroupMessage(peerId, { from: $user.user_id, text: '', ts, status: 'sent', group_id: peerId, sender_id: $user.user_id, location: locObj, reply_to_ts: null, reply_to_from: null, reply_to_text: null, file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null })
      }
      await tick(); scrollBottom()
    } catch (e) { error = String(e) }
  }

  // ── Translation ───────────────────────────────────────────────────────────────

  async function translateMsg(m) {
    const key = `${m.ts}_${m.from}`
    if (translations[key]) {
      const t = { ...translations }; delete t[key]; translations = t
      return
    }
    translating = { ...translating, [key]: true }
    try {
      const lang = (navigator.language ?? 'en').split('-')[0]
      const result = await invoke('translate_message', { text: m.text ?? '', targetLang: lang })
      translations = { ...translations, [key]: result }
    } catch (e) {
      error = String(e)
    } finally {
      translating = { ...translating, [key]: false }
    }
  }

  // ── RSVP ─────────────────────────────────────────────────────────────────────

  async function sendRsvp(eventTs, status) {
    const gid = isChannel ? channelGroupId : peerId
    try {
      await invoke('send_rsvp', { groupId: gid, eventTs, status })
    } catch (e) { error = String(e) }
  }

  // ── C6 Scheduling helpers ─────────────────────────────────────────────────────

  async function scheduleMessage() {
    if (!scheduleDatetime || !text.trim()) return
    const sendAtMs = new Date(scheduleDatetime).getTime()
    if (sendAtMs <= Date.now()) { error = 'Please pick a future time'; return }
    try {
      await invoke('schedule_message', {
        peerId: convKey,
        text: text.trim(),
        sendAtMs,
        isGroup: isGroup || isChannel,
        isChannel,
        channelGroupId: isChannel ? channelGroupId : null,
        replyTo: replyTo ? { ts: replyTo.ts, from: replyTo.from, text: replyTo.text } : null,
        mentions: mentions.length ? mentions : null,
      })
      text = ''
      replyTo = null
      mentions = []
      scheduleDatetime = ''
      showScheduleModal = false
      scheduledMsgs = await invoke('list_scheduled', { peerId: convKey }).catch(() => [])
    } catch (e) { error = String(e) }
  }

  async function cancelScheduled(id) {
    scheduledMsgs = scheduledMsgs.filter(s => s.id !== id)
    await invoke('cancel_scheduled', { id }).catch(() => {})
    scheduledMsgs = await invoke('list_scheduled', { peerId: convKey }).catch(() => [])
  }

  function fmtScheduledTime(ms) {
    const d = new Date(ms)
    return d.toLocaleDateString([], { month: 'short', day: 'numeric' }) + ' ' +
           d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }

  // ── Slash commands ────────────────────────────────────────────────────────────

  let showSlashPalette = false
  let slashQuery = ''
  let slashPaletteRef

  // ── Chat background ──────────────────────────────────────────────────────────
  $: bgImageData = $chatBg.type === 'image' ? (localStorage.getItem('chat_bg_image') ?? '') : ''
  $: chatBgStyle = (() => {
    const { type, gradient, blur } = $chatBg
    const blurStyle = blur > 0 ? `filter:blur(${blur}px);transform:scale(1.08);` : ''
    if (type === 'gradient') return `background:${CHAT_GRADIENTS[gradient]?.css ?? ''};${blurStyle}`
    if (type === 'image' && bgImageData) return `background:url('${bgImageData}') center/cover no-repeat;${blurStyle}`
    return ''
  })()

  // ── Long-press send (C4) ─────────────────────────────────────────────────────
  let sendMenuOpen = false
  let sendLongTimer = null

  function onSendPointerDown(e) {
    if (e.button !== 0) return
    sendLongTimer = setTimeout(() => { sendLongTimer = null; sendMenuOpen = true }, 500)
  }
  function onSendPointerUp() {
    if (sendLongTimer !== null) { clearTimeout(sendLongTimer); sendLongTimer = null; send() }
  }
  function onSendPointerCancel() {
    if (sendLongTimer !== null) { clearTimeout(sendLongTimer); sendLongTimer = null }
  }

  // ── /help modal ──────────────────────────────────────────────────────────────
  let showHelpModal = false
  const HELP_COMMANDS = [
    { icon: '🗑',  cmd: 'clear',    desc: 'Delete all local messages' },
    { icon: '🔕',  cmd: 'mute',     desc: 'Mute: 1h | 8h | 1w | off' },
    { icon: '⏱',  cmd: 'ttl',      desc: 'Disappearing messages: off | 1h | 1d | 1w' },
    { icon: '⬇',  cmd: 'export',   desc: 'Export chat: json | html | md' },
    { icon: '🔍',  cmd: 'search',   desc: 'Search in conversation' },
    { icon: '📊',  cmd: 'poll',     desc: 'Create a poll (groups only)' },
    { icon: '🕐',  cmd: 'schedule', desc: 'Send a message at a specific time' },
    { icon: '❓',  cmd: 'help',     desc: 'Show this command reference' },
  ]

  async function executeSlashCommand(cmd) {
    showSlashPalette = false
    text = ''
    switch (cmd.cmd) {
      case 'clear':
        if (confirm('Delete all local messages? This cannot be undone.')) {
          await invoke('delete_message', { peerId, ts: 0, from: '', deleteAll: true }).catch(() => {})
          conversations.update(c => ({ ...c, [peerId]: [] }))
        }
        break
      case 'mute':
        showMoreMenu = true
        break
      case 'ttl':
        showMoreMenu = true
        break
      case 'export':
        showExport = true
        break
      case 'search':
        showSearch.set(true)
        break
      case 'poll':
        showPollModal = true
        break
      case 'schedule':
        showScheduleModal = true
        break
      case 'help':
        showHelpModal = true
        break
    }
    await tick()
    textareaEl?.focus()
  }

  function onKeydown(e) {
    if (showSlashPalette) {
      if (e.key === 'Escape') { showSlashPalette = false; return }
      if (e.key === 'ArrowDown') { e.preventDefault(); slashPaletteRef?.moveDown(); return }
      if (e.key === 'ArrowUp') { e.preventDefault(); slashPaletteRef?.moveUp(); return }
      if (e.key === 'Enter') { e.preventDefault(); slashPaletteRef?.confirm(); return }
    }
    if (showMentionPicker && (e.key === 'Escape' || e.key === 'ArrowDown')) {
      if (e.key === 'Escape') { showMentionPicker = false; return }
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      if (showMentionPicker && mentionCandidates.length > 0) {
        selectMention(mentionCandidates[0])
        return
      }
      send()
    }
  }

  // ── @mention autocomplete ──────────────────────────────────────────────────

  let mentions = []             // { id: string, name: string }[] for the current message
  let showMentionPicker = false
  let mentionSearch = ''

  $: memberList = isGroup
    ? ($groups[peerId]?.members ?? []).filter(m => m.user_id !== $user?.user_id)
    : []
  $: mentionCandidates = showMentionPicker
    ? memberList.filter(m => m.username.toLowerCase().includes(mentionSearch.toLowerCase()))
    : []

  function selectMention(member) {
    if (!textareaEl) return
    const cursor = textareaEl.selectionStart
    const before = text.slice(0, cursor)
    const after = text.slice(cursor)
    const match = before.match(/@(\w*)$/)
    if (match) {
      text = before.slice(0, -match[0].length) + '@' + member.username + ' ' + after
      if (!mentions.some(m => m.id === member.user_id)) {
        mentions = [...mentions, { id: member.user_id, name: member.username }]
      }
    }
    showMentionPicker = false
    tick().then(() => textareaEl?.focus())
  }

  function renderText(m) {
    const raw = m.text ?? ''
    if (!raw) return ''
    if (!m.mentions?.length) return renderMd(raw)
    const mentionedIds = new Set(m.mentions)
    const members = $groups[peerId]?.members ?? []
    const mentionNames = {}
    for (const mb of members) {
      if (mentionedIds.has(mb.user_id)) {
        mentionNames[mb.username] = mb.user_id === $user?.user_id
      }
    }
    if (!Object.keys(mentionNames).length) return renderMd(raw)
    const processed = raw.replace(/@(\w+)/g, (match, name) => {
      if (name in mentionNames) {
        return `<mark class="mention${mentionNames[name] ? ' mention-mine' : ''}">${match}</mark>`
      }
      return match
    })
    return renderMd(processed)
  }

  // ── Typing indicator ───────────────────────────────────────────────────────

  let typingTimer = null

  function onInput() {
    scheduleDraftSave(peerId, text)
    // Slash command palette: '/' at position 0 in otherwise empty textarea
    if (text.startsWith('/') && !text.includes(' ') && !text.includes('\n')) {
      slashQuery = text.slice(1)
      showSlashPalette = true
    } else {
      showSlashPalette = false
    }
    // @mention autocomplete (groups only)
    if (isGroup && textareaEl) {
      const cursor = textareaEl.selectionStart
      const atMatch = text.slice(0, cursor).match(/@(\w*)$/)
      if (atMatch) {
        mentionSearch = atMatch[1]
        showMentionPicker = true
      } else {
        showMentionPicker = false
      }
    }
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

  // ── Native context menu ────────────────────────────────────────────────────

  // ── Channel message event ─────────────────────────────────────────────────────
  let unlistenChanMsg
  onMount(async () => {
    unlistenChanMsg = await listen('channel_message', ({ payload }) => {
      const { gid, cid, from, text: t, ts, sticker, location: loc, event_data, rsvp } = payload
      addGroupMessage(cid, {
        from, text: t ?? '', ts, status: 'received',
        group_id: cid, sender_id: from,
        sticker: sticker ?? null,
        location: loc ?? null,
        event_data: event_data ?? null,
        rsvp: rsvp ?? null,
        reply_to_ts: null, reply_to_from: null, reply_to_text: null,
        file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null,
      })
      const chanConvId = `${gid}/${cid}`
      if ($activeConv !== chanConvId) {
        unreadCounts.update(c => ({ ...c, [cid]: (c[cid] ?? 0) + 1 }))
      }
    })
  })
  onDestroy(() => { unlistenChanMsg?.() })

  let unlistenCtxMenu
  onMount(async () => {
    unlistenCtxMenu = await listen('ctx_menu_action', ({ payload }) => {
      const { action, ctx } = payload
      if (action === 'ctx_copy') {
        navigator.clipboard.writeText(ctx.text)
      } else if (action === 'ctx_pin') {
        togglePin({ ts: ctx.ts, from: ctx.from })
      } else if (action === 'ctx_save_note') {
        invoke('save_note', { text: ctx.text }).catch(console.error)
      } else if (action === 'ctx_delete_me') {
        invoke('delete_message', { peerId: ctx.peer_id, msgTs: ctx.ts, forAll: false }).catch(console.error)
        removeMessage(ctx.peer_id, ctx.ts, ctx.from)
      } else if (action === 'ctx_delete_all') {
        invoke('delete_message', { peerId: ctx.peer_id, msgTs: ctx.ts, forAll: true }).catch(console.error)
        removeMessage(ctx.peer_id, ctx.ts, ctx.from)
      }
    })
  })
  onDestroy(() => { unlistenCtxMenu?.() })

  let unlistenSched
  onMount(async () => {
    unlistenSched = await listen('scheduled_sent', ({ payload }) => {
      if (payload.peer_id === convKey) {
        invoke('list_scheduled', { peerId: convKey }).then(l => { scheduledMsgs = l }).catch(() => {})
      }
    })
  })
  onDestroy(() => { unlistenSched?.() })

  // ── B10 Focus trap helper ──────────────────────────────────────────────────
  function trapFocus(e) {
    if (e.key !== 'Tab') return
    const modal = e.currentTarget
    const focusable = modal.querySelectorAll(
      'button,[href],input,select,textarea,[tabindex]:not([tabindex="-1"])'
    )
    if (!focusable.length) return
    const first = focusable[0], last = focusable[focusable.length - 1]
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus() }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus() }
  }
</script>

<svelte:window
  on:click={() => { showMoreMenu = false; showStickerPicker = false; showAttachMenu = false; sendMenuOpen = false }}
  on:keydown={e => e.key === 'Escape' && (showMoreMenu = false, showSlashPalette = false, showMentionPicker = false, cancelEdit(), showEditHistory = null, pickerVisible = {}, showStickerPicker = false, showAttachMenu = false, showEventModal = false, showScheduleModal = false, sendMenuOpen = false, showHelpModal = false, openThread = null)}
/>

<div class="pane" class:thread-open={openThread !== null}>
<div class="pane-main">
  <div class="header">
    {#if onToggleSidebar}
      <button class="burger-btn" on:click={onToggleSidebar} aria-label="Open sidebar">☰</button>
    {/if}
    {#if isSaved}
      <span class="saved-header-icon">🔖</span>
      <div class="header-info">
        <span class="peer-name">Saved Messages</span>
        <span class="peer-id">Your personal notes</span>
      </div>
    {:else if isChannel}
      <span class="group-header-icon">#</span>
      <div class="header-info">
        <span class="peer-name">#{channelInfo?.name ?? channelId?.slice(0, 8) + '…'}</span>
        <span class="peer-id">{channelGroupInfo?.name ?? ''}</span>
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
        aria-label="Verify safety number"
        on:click={() => showSafetyNumbers = true}
      >🔒</button>
    {/if}

    <div class="header-extras">
      {#if onToggleDetail}
        <button
          class="icon-hdr-btn"
          class:active={detailOpen}
          title={detailOpen ? 'Hide info panel' : 'Show info panel'}
          aria-label={detailOpen ? 'Hide info panel' : 'Show info panel'}
          on:click={onToggleDetail}
        >ℹ</button>
      {/if}
      {#if !isSaved}
        <div class="popover-wrap">
          <button
            class="icon-hdr-btn"
            class:active-mute={muteSettings.is_muted}
            class:active-ttl={ttl > 0 || retentionCount > 0}
            title="Conversation settings"
            aria-label="Conversation settings"
            on:click|stopPropagation={() => showMoreMenu = !showMoreMenu}
          >⋯</button>
          {#if showMoreMenu}
            <div class="conv-settings-menu" role="menu" tabindex="0" on:click|stopPropagation on:keydown|stopPropagation>
              <!-- Mute row -->
              <div class="csm-row">
                <span class="csm-label">🔔</span>
                <div class="csm-pills">
                  <button class="csm-pill" class:selected={!muteSettings.is_muted} on:mousedown|preventDefault on:click={() => setMute(0)}>On</button>
                  <button class="csm-pill" class:selected={muteSettings.is_muted} on:mousedown|preventDefault on:click={() => setMute(1)}>1h</button>
                  <button class="csm-pill" on:mousedown|preventDefault on:click={() => setMute(8)}>8h</button>
                  <button class="csm-pill" on:mousedown|preventDefault on:click={() => setMute(168)}>1w</button>
                  <button class="csm-pill" on:mousedown|preventDefault on:click={() => setMute(null)}>∞</button>
                </div>
              </div>
              <!-- TTL row -->
              <div class="csm-row">
                <span class="csm-label">⏱</span>
                <div class="csm-pills">
                  <button class="csm-pill" class:selected={ttl===0} on:mousedown|preventDefault on:click={() => setTtl(0)}>Off</button>
                  <button class="csm-pill" class:selected={ttl===86400} on:mousedown|preventDefault on:click={() => setTtl(86400)}>24h</button>
                  <button class="csm-pill" class:selected={ttl===604800} on:mousedown|preventDefault on:click={() => setTtl(604800)}>7d</button>
                  <button class="csm-pill" class:selected={ttl===2592000} on:mousedown|preventDefault on:click={() => setTtl(2592000)}>30d</button>
                </div>
              </div>
              <!-- Retention row -->
              <div class="csm-row">
                <span class="csm-label">♻</span>
                <div class="csm-pills">
                  <button class="csm-pill" class:selected={retentionCount===0} on:mousedown|preventDefault on:click={() => setRetention(0)}>All</button>
                  <button class="csm-pill" class:selected={retentionCount===500} on:mousedown|preventDefault on:click={() => setRetention(500)}>500</button>
                  <button class="csm-pill" class:selected={retentionCount===1000} on:mousedown|preventDefault on:click={() => setRetention(1000)}>1K</button>
                  <button class="csm-pill" class:selected={retentionCount===5000} on:mousedown|preventDefault on:click={() => setRetention(5000)}>5K</button>
                </div>
              </div>
              <div class="csm-divider"></div>
              <button class="csm-action" on:click={() => { showExport = true; showMoreMenu = false }}>⬇ Export chat…</button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </div>

  <!-- Pinned messages banner -->
  {#if pinnedMsgs.length > 0}
    <div class="pinned-banner" role="button" tabindex="0"
      on:click|stopPropagation={() => {
        if (pinnedMsgs.length === 1) scrollToPinned(pinnedMsgs[0])
        else showPinnedList = !showPinnedList
      }}
      on:keydown={e => (e.key === 'Enter' || e.key === ' ') && (pinnedMsgs.length === 1 ? scrollToPinned(pinnedMsgs[0]) : (showPinnedList = !showPinnedList))}
    >
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
  <!-- C15 Retention banner -->
  {#if retentionCount > 0}
    <div class="ttl-banner">♻ Keeping last {retentionCount.toLocaleString()} messages</div>
  {/if}

  <!-- Scheduled messages banner -->
  {#if scheduledMsgs.length > 0}
    <div class="sched-banner" on:click={() => showScheduledList = !showScheduledList} role="button" tabindex="0" aria-expanded={showScheduledList} on:keydown={e => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), showScheduledList = !showScheduledList)}>
      🕐 {scheduledMsgs.length} scheduled {scheduledMsgs.length === 1 ? 'message' : 'messages'}
      <span class="sched-chevron">{showScheduledList ? '▲' : '▼'}</span>
    </div>
    {#if showScheduledList}
      <div class="sched-list">
        {#each scheduledMsgs as sm (sm.id)}
          <div class="sched-item">
            <span class="sched-time">{fmtScheduledTime(sm.send_at_ms)}</span>
            <span class="sched-text">{sm.text.length > 55 ? sm.text.slice(0, 55) + '…' : sm.text}</span>
            <button class="sched-cancel-btn" title="Cancel scheduled message" aria-label="Cancel scheduled message" on:click|stopPropagation={() => cancelScheduled(sm.id)}>✕</button>
          </div>
        {/each}
      </div>
    {/if}
  {/if}

  <div
    class="messages-wrap"
    class:drag-over={dragging}
    role="region"
    aria-label="Messages"
    on:dragover={onDragOver}
    on:dragleave={onDragLeave}
    on:drop={onDrop}
  >
  {#if $chatBg.type !== 'none' && chatBgStyle}
    <div class="chat-bg-layer" style={chatBgStyle} aria-hidden="true">
      {#if $chatBg.dim > 0}
        <div class="chat-bg-dim" style="opacity:{$chatBg.dim / 100}"></div>
      {/if}
    </div>
  {/if}
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
      <div class="msg" class:mine role="listitem" data-ts={m.ts} on:contextmenu={e => {
        e.preventDefault()
        const isPinned = pinnedMsgs.some(p => p.msg_ts === m.ts && p.msg_from === m.from)
        invoke('show_message_context_menu', { ctx: { peer_id: peerId, ts: m.ts, from: m.from, text: m.text ?? '', mine, is_pinned: isPinned } }).catch(console.error)
      }}>
        {#if !mine}
          <Avatar name={senderName} uid={isGroup ? m.from : peerId} size={24} />
        {/if}
        <div class="msg-col" class:mine>
          <div
            class="bubble md-content"
            class:mine
            role="group"
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
                    <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
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
            {:else if m.sticker}
              <div class="sticker-bubble">{m.sticker.id}</div>
            {:else if m.location}
              <div class="location-bubble">
                <!-- svelte-ignore a11y-img-redundant-alt -->
                <img
                  class="location-map"
                  src="https://staticmap.openstreetmap.de/staticmap.php?center={m.location.lat},{m.location.lng}&zoom=14&size=300x150&markers={m.location.lat},{m.location.lng}"
                  alt="Map"
                  loading="lazy"
                />
                <div class="location-footer">📍 Location{m.location.acc ? ` (±${m.location.acc}m)` : ''}</div>
              </div>
            {:else if m.event_data}
              <div class="event-bubble">
                <div class="event-title">📅 {m.event_data.title}</div>
                <div class="event-date">{new Date(m.event_data.date_ms).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })}</div>
                {#if m.event_data.location}<div class="event-loc">📍 {m.event_data.location}</div>{/if}
                {#if m.event_data.desc}<div class="event-desc">{m.event_data.desc}</div>{/if}
                <div class="event-rsvp-btns">
                  <button class="rsvp-btn yes" on:click={() => sendRsvp(m.event_data.date_ms, 'yes')}>✅ Going</button>
                  <button class="rsvp-btn maybe" on:click={() => sendRsvp(m.event_data.date_ms, 'maybe')}>❓ Maybe</button>
                  <button class="rsvp-btn no" on:click={() => sendRsvp(m.event_data.date_ms, 'no')}>❌ No</button>
                </div>
              </div>
            {:else if m.rsvp}
              <div class="rsvp-bubble">
                <span class="rsvp-icon">{m.rsvp.status === 'yes' ? '✅' : m.rsvp.status === 'no' ? '❌' : '❓'}</span>
                <span class="rsvp-label">{m.rsvp.status === 'yes' ? 'Going' : m.rsvp.status === 'no' ? 'Not going' : 'Maybe'}</span>
              </div>
            {:else if parsePollId(m.text)}
              <PollView pollId={parsePollId(m.text)} {mine} />
            {:else}
              {@html renderText(m)}
              {#if translations[msgKey]}
                <div class="translation-text">{translations[msgKey]}</div>
              {/if}
              {#if linkPreviews[msgKey] && linkPreviews[msgKey] !== 'loading'}
                <LinkPreviewCard preview={linkPreviews[msgKey]} />
              {/if}
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
                role="toolbar"
                aria-label="Quick reactions"
                tabindex="0"
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

          {#if isGroup && !m.thread_parent_ts && m.thread_reply_count > 0}
            <button
              class="thread-count-badge"
              on:click={() => openThread = { ts: m.ts, from: m.from ?? m.sender_id, text: m.text }}
              title="View thread"
            >↳ {m.thread_reply_count} {m.thread_reply_count === 1 ? 'reply' : 'replies'}</button>
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
          {#if isGroup && !m.thread_parent_ts}
            <button
              class="thread-btn"
              title="Reply in thread"
              aria-label="Reply in thread"
              on:click={() => openThread = { ts: m.ts, from: m.from ?? m.sender_id, text: m.text }}
            >↳</button>
          {/if}
          {#if mine && !m.file_id && !m.sticker && !m.location && !m.event_data && !m.rsvp}
            <button
              class="edit-btn"
              title="Edit message"
              on:click={() => startEdit(m)}
            >✏</button>
          {/if}
          {#if m.text && !m.file_id && !m.sticker && !m.location && !m.event_data && !m.rsvp}
            <button
              class="translate-btn"
              title={translations[msgKey] ? 'Show original' : 'Translate'}
              aria-label={translations[msgKey] ? 'Show original message' : 'Translate message'}
              on:click={() => translateMsg(m)}
            >{translating[msgKey] ? '…' : '🌐'}</button>
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
    <button class="scroll-btn" on:click={scrollBottom} title="Scroll to bottom" aria-label="Scroll to bottom">↓</button>
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

  {#if showSlashPalette}
    <SlashCommandPalette
      bind:this={slashPaletteRef}
      query={slashQuery}
      {isGroup}
      onSelect={executeSlashCommand}
    />
  {/if}

  {#if showMentionPicker && mentionCandidates.length > 0}
    <div class="mention-picker" role="listbox" aria-label="Mention suggestions">
      {#each mentionCandidates.slice(0, 8) as member (member.user_id)}
        <button
          type="button"
          class="mention-item"
          role="option"
          aria-selected={false}
          on:mousedown|preventDefault={() => selectMention(member)}
        >@{member.username}</button>
      {/each}
    </div>
  {/if}

  {#if showStickerPicker}
    <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
    <div class="sticker-picker-wrap" on:click|stopPropagation>
      <StickerPicker
        onSelect={(s) => sendSticker(s)}
        onClose={() => showStickerPicker = false}
      />
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
      title="Stickers"
      aria-label="Stickers"
      disabled={sending}
      on:mousedown|preventDefault
      on:click|stopPropagation={() => { showStickerPicker = !showStickerPicker; showAttachMenu = false }}
    >😊</button>
    <div class="attach-menu-wrap" role="none">
      <button
        type="button"
        class="attach-btn"
        title="Attach"
        aria-label="Attach"
        disabled={sending}
        on:mousedown|preventDefault
        on:click|stopPropagation={() => { showAttachMenu = !showAttachMenu; showStickerPicker = false }}
      >📎</button>
      {#if showAttachMenu}
      <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
      <div class="attach-popup" role="menu" tabindex="0" on:click|stopPropagation>
        <button class="attach-item" role="menuitem" on:mousedown|preventDefault on:click={() => { attachFile(); showAttachMenu = false }}>
          <span class="attach-item-ico">📄</span><span>File</span>
        </button>
        <button class="attach-item" role="menuitem" on:mousedown|preventDefault on:click={() => { showPollModal = true; showAttachMenu = false }}>
          <span class="attach-item-ico">📊</span><span>Poll</span>
        </button>
        <button class="attach-item" role="menuitem" on:mousedown|preventDefault on:click={() => { sendLocation(); showAttachMenu = false }}>
          <span class="attach-item-ico">📍</span><span>Location</span>
        </button>
        <button class="attach-item" role="menuitem" on:mousedown|preventDefault on:click={() => { showEventModal = true; showAttachMenu = false }}>
          <span class="attach-item-ico">📅</span><span>Event</span>
        </button>
      </div>
      {/if}
    </div>
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
      aria-label="Record voice message"
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
      <button type="button" class="rec-stop-btn" aria-label="Send recording" on:click={() => { clearInterval(recTicker); recTicker = null; stopAndSendVoice(false) }}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
      </button>
    {/if}
    {#if recState === 'idle' && text.trim() && !isSaved}
      <button type="button" class="sched-btn" title="Schedule for later" aria-label="Schedule message for later" on:mousedown|preventDefault on:click={() => showScheduleModal = true}>🕐</button>
    {/if}
    {#if recState === 'idle'}
      <div class="send-wrap">
        <button
          class="send-btn"
          type="button"
          disabled={sending || !text.trim()}
          title="Send (Enter) · Hold to schedule"
          aria-label="Send message (hold to schedule)"
          on:pointerdown={onSendPointerDown}
          on:pointerup={onSendPointerUp}
          on:pointerleave={onSendPointerCancel}
          on:pointercancel={onSendPointerCancel}
          on:contextmenu|preventDefault
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
        </button>
        {#if sendMenuOpen}
          <div class="send-ctx-menu" role="menu" tabindex="0" on:click|stopPropagation on:keydown|stopPropagation>
            <button role="menuitem" on:click={() => { sendMenuOpen = false; send() }}>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
              Send now
            </button>
            <button role="menuitem" on:click={() => { sendMenuOpen = false; showScheduleModal = true }}>
              🕐 Schedule…
            </button>
          </div>
        {/if}
      </div>
    {/if}
  </form>

  {#if error}
    <p class="err">{error}</p>
  {/if}
</div>

{#if openThread}
  <ThreadPane
    groupId={peerId}
    parentMsg={openThread}
    onClose={() => openThread = null}
  />
{/if}
</div>

{#if showHelpModal}
  <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
  <div class="help-overlay" role="none" on:click|self={() => showHelpModal = false} on:keydown|self={e => e.key === 'Escape' && (showHelpModal = false)}>
    <div class="help-box" role="dialog" aria-modal="true" aria-label="Slash command reference">
      <div class="help-hdr">
        <span>Slash commands</span>
        <button class="help-close" on:click={() => showHelpModal = false} aria-label="Close">✕</button>
      </div>
      <div class="help-list">
        {#each HELP_COMMANDS as c}
          <div class="help-item">
            <span class="help-icon">{c.icon}</span>
            <span class="help-cmd">/{c.cmd}</span>
            <span class="help-desc">{c.desc}</span>
          </div>
        {/each}
      </div>
      <div class="help-hint">Type <kbd>/</kbd> in the message box to use a command</div>
    </div>
  </div>
{/if}

{#if lightboxUrl}
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
  <div class="lightbox-overlay" role="dialog" aria-modal="true" aria-label="Image viewer"
    on:click={() => { URL.revokeObjectURL(lightboxUrl); lightboxUrl = null }}
    on:keydown={e => { if (e.key === 'Escape') { URL.revokeObjectURL(lightboxUrl); lightboxUrl = null } }}>
    <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
    <img class="lightbox-img" src={lightboxUrl} alt="full size" on:click|stopPropagation />
    <button class="lightbox-close" aria-label="Close image" on:click={() => { URL.revokeObjectURL(lightboxUrl); lightboxUrl = null }}>✕</button>
  </div>
{/if}

<!-- Poll creation modal -->
{#if showPollModal}
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="poll-modal-title" on:click|self={() => showPollModal = false}>
    <div class="modal-box poll-modal" role="document" on:keydown={trapFocus}>
      <div class="modal-title" id="poll-modal-title">Create Poll</div>
      <input
        class="modal-input"
        type="text"
        bind:value={pollQuestion}
        placeholder="Question…"
        maxlength="200"
      />
      <div class="poll-opts-list">
        {#each pollOptions as _, i}
          <div class="poll-opt-row">
            <input
              class="modal-input poll-opt-input"
              type="text"
              bind:value={pollOptions[i]}
              placeholder="Option {i + 1}"
              maxlength="100"
            />
            {#if pollOptions.length > 2}
              <button class="poll-remove-btn" on:click={() => removePollOption(i)}>✕</button>
            {/if}
          </div>
        {/each}
      </div>
      {#if pollOptions.length < 10}
        <button class="poll-add-opt-btn" on:click={addPollOption}>+ Add option</button>
      {/if}
      <div class="modal-btns">
        <button
          class="modal-ok"
          on:click={createAndSendPoll}
          disabled={creatingPoll || !pollQuestion.trim() || pollOptions.filter(o => o.trim()).length < 2}
        >{creatingPoll ? 'Creating…' : 'Create'}</button>
        <button class="modal-cancel" on:click={() => showPollModal = false}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<!-- Export chat dialog -->
{#if showExport}
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="export-modal-title" on:click|self={() => showExport = false}>
    <div class="modal-box" role="document" on:keydown={trapFocus}>
      <div class="modal-title" id="export-modal-title">Export chat</div>
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
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="edit-history-title" on:click|self={() => showEditHistory = null}>
    <div class="modal-box" role="document" on:keydown={trapFocus}>
      <div class="modal-title" id="edit-history-title">Edit history</div>
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

{#if showEventModal && (isGroup || isChannel)}
  <GroupEventModal
    groupId={isChannel ? channelGroupId : peerId}
    on:sent={() => { showEventModal = false; tick().then(scrollBottom) }}
    on:close={() => showEventModal = false}
  />
{/if}

<!-- C6 Schedule modal -->
{#if showScheduleModal && !isSaved}
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
  <div class="modal-overlay" on:click|self={() => showScheduleModal = false} role="dialog" aria-modal="true" aria-labelledby="schedule-modal-title">
    <div class="modal-box" role="document" on:keydown={trapFocus}>
      <div class="modal-title" id="schedule-modal-title">🕐 Schedule Message</div>
      <div class="sched-preview">{text.trim() || '(type a message first)'}</div>
      <input
        type="datetime-local"
        class="modal-input"
        bind:value={scheduleDatetime}
        min={new Date(Date.now() + 60000).toISOString().slice(0, 16)}
      />
      <div class="modal-btns">
        <button class="modal-ok" disabled={!scheduleDatetime || !text.trim()} on:click={scheduleMessage}>Schedule</button>
        <button class="modal-cancel" on:click={() => showScheduleModal = false}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<!-- Video recording modal -->
{#if showVideoModal}
  <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
  <div class="modal-overlay video-modal-overlay" role="dialog" aria-modal="true" aria-labelledby="video-modal-title" on:click|self={() => closeVideoModal(true)}>
    <div class="video-modal-box">
      <div class="video-modal-title" id="video-modal-title">Video message</div>
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
    position: relative;
  }

  .header {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .burger-btn {
    background: none;
    color: var(--text-muted);
    font-size: 18px;
    padding: 2px 6px;
    line-height: 1;
    border-radius: 6px;
    flex-shrink: 0;
  }
  .burger-btn:hover { background: var(--bg-hover); color: var(--text); }
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

  /* ── Chat background layer ───────────────────────────────────────────────── */
  .chat-bg-layer {
    position: absolute;
    inset: 0;
    z-index: 0;
    overflow: hidden;
    pointer-events: none;
  }
  .chat-bg-dim {
    position: absolute;
    inset: 0;
    background: #000;
    pointer-events: none;
  }

  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: 6px;
    position: relative;
    z-index: 1;
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

  .attach-menu-wrap { position: relative; }
  .attach-popup {
    position: absolute;
    bottom: calc(100% + 8px);
    left: 0;
    z-index: 60;
    background: var(--bg-2, #1e1e2e);
    border: 1px solid var(--border, #313244);
    border-radius: 10px;
    box-shadow: 0 8px 28px rgba(0,0,0,0.45);
    padding: 6px;
    min-width: 160px;
  }
  .attach-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    background: none;
    color: var(--text);
    text-align: left;
    padding: 8px 12px;
    font-size: 13px;
    border-radius: 6px;
    cursor: pointer;
  }
  .attach-item:hover { background: var(--bg-hover); }
  .attach-item-ico {
    font-size: 18px;
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-active, rgba(137,180,250,0.12));
    border-radius: 8px;
    flex-shrink: 0;
  }

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

  /* ── Group read receipts ─────────────────────────────────────────────────── */
  .grp-read-row {
    display: flex; align-items: center; gap: 3px;
    margin-top: 2px; cursor: default;
  }
  .grp-read-label { font-size: 11px; color: #4caf7d; }
  .grp-read-names { font-size: 10px; color: var(--text-muted); }

  /* ── Header extras ───────────────────────────────────────────────────────── */
  .header-extras { display: flex; align-items: center; gap: 4px; margin-left: auto; flex-shrink: 0; }
  .icon-hdr-btn {
    background: none; border: none; cursor: pointer;
    color: var(--text-muted); font-size: 14px; padding: 3px 5px; border-radius: 4px;
  }
  .icon-hdr-btn:hover { background: var(--bg-hover); color: var(--text); }
  .icon-hdr-btn.active { background: var(--bg-active); color: var(--accent); }
  .icon-hdr-btn.active-mute { color: var(--danger, #f38ba8); }
  .icon-hdr-btn.active-ttl  { color: var(--accent); }

  /* ── Conversation settings menu (⋯) ─────────────────────────────────────── */
  .conv-settings-menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 50;
    background: var(--bg-2, #1e1e2e);
    border: 1px solid var(--border, #313244);
    border-radius: 10px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    padding: 8px;
    min-width: 260px;
  }
  .csm-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 0;
  }
  .csm-label { font-size: 14px; flex-shrink: 0; width: 22px; text-align: center; }
  .csm-pills { display: flex; gap: 4px; flex-wrap: wrap; }
  .csm-pill {
    background: none;
    color: var(--text-muted);
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 12px;
    border: 1px solid var(--border);
    cursor: pointer;
    transition: background 0.12s, color 0.12s;
  }
  .csm-pill:hover { background: var(--bg-hover); color: var(--text); }
  .csm-pill.selected { background: var(--accent); color: #fff; border-color: var(--accent); }
  .csm-divider { height: 1px; background: var(--border); margin: 6px 0; }
  .csm-action {
    display: block; width: 100%; background: none; color: var(--text-muted);
    text-align: left; font-size: 12px; padding: 5px 8px; border-radius: 6px; cursor: pointer;
    border: none;
  }
  .csm-action:hover { background: var(--bg-hover); color: var(--text); }

  /* ── Popovers ─────────────────────────────────────────────────────────────── */
  .popover-wrap { position: relative; }

  /* ── TTL banners ──────────────────────────────────────────────────────────── */
  .ttl-banner {
    padding: 5px 16px; font-size: 11px; text-align: center;
    background: rgba(137,180,250,0.08); color: var(--accent);
  }
  .expiring-banner {
    padding: 5px 16px; font-size: 11px; text-align: center;
    background: rgba(243,139,168,0.1); color: #f38ba8;
  }

  /* ── C6 Scheduling ──────────────────────────────────────────────────────────── */
  .sched-btn { background: none; font-size: 18px; padding: 4px 6px; color: var(--text-muted, #9399b2); line-height: 1; border: none; cursor: pointer; border-radius: 4px; }
  .sched-btn:hover { background: var(--bg-hover, rgba(137,180,250,0.1)); color: var(--accent, #89b4fa); }
  .sched-banner { display: flex; align-items: center; gap: 6px; padding: 5px 16px; background: color-mix(in srgb, var(--accent, #89b4fa) 8%, transparent); font-size: 12px; color: var(--text, #cdd6f4); cursor: pointer; border-bottom: 1px solid var(--border, rgba(255,255,255,0.06)); user-select: none; }
  .sched-chevron { margin-left: auto; font-size: 10px; color: var(--text-muted, #9399b2); }
  .sched-list { border-bottom: 1px solid var(--border, rgba(255,255,255,0.06)); }
  .sched-item { display: flex; align-items: center; gap: 8px; padding: 6px 16px; font-size: 12px; border-top: 1px solid var(--border, rgba(255,255,255,0.06)); }
  .sched-time { color: var(--accent, #89b4fa); white-space: nowrap; flex-shrink: 0; }
  .sched-text { flex: 1; color: var(--text-muted, #9399b2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sched-cancel-btn { background: none; color: var(--text-muted, #9399b2); font-size: 12px; padding: 2px 6px; border-radius: 4px; border: none; cursor: pointer; flex-shrink: 0; }
  .sched-cancel-btn:hover { background: rgba(243,139,168,0.2); color: #f38ba8; }
  .sched-preview { padding: 8px 10px; background: var(--bg, #1e1e2e); border-radius: 6px; font-size: 13px; color: var(--text-muted, #9399b2); font-style: italic; max-height: 60px; overflow: hidden; text-overflow: ellipsis; margin-bottom: 12px; }

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
  .poll-modal { min-width: 280px; }
  .poll-opts-list { display: flex; flex-direction: column; gap: 6px; margin-bottom: 8px; }
  .poll-opt-row { display: flex; gap: 6px; align-items: center; }
  .poll-opt-input { flex: 1; margin-bottom: 0 !important; }
  .poll-remove-btn { background: none; border: none; color: var(--text-muted); cursor: pointer; font-size: 13px; padding: 0 4px; flex-shrink: 0; }
  .poll-remove-btn:hover { color: #e05555; }
  .poll-add-opt-btn { background: none; border: 1px dashed var(--border); color: var(--accent); font-size: 11px; padding: 5px 10px; border-radius: 6px; cursor: pointer; width: 100%; margin-bottom: 10px; }
  .poll-add-opt-btn:hover { background: var(--bg-hover); }

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

  /* ── Send button ─────────────────────────────────────────────────────────── */
  /* ── Send long-press menu ────────────────────────────────────────────────── */
  .send-wrap { position: relative; display: flex; align-items: center; }
  .send-ctx-menu {
    position: absolute;
    bottom: calc(100% + 6px);
    right: 0;
    z-index: 60;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 20px rgba(0,0,0,0.35);
    overflow: hidden;
    min-width: 140px;
  }
  .send-ctx-menu button {
    display: flex; align-items: center; gap: 7px;
    width: 100%; padding: 9px 13px;
    background: none; color: var(--text); font-size: 13px;
    border-radius: 0; text-align: left;
    transition: background 0.1s;
  }
  .send-ctx-menu button:hover { background: var(--bg-hover); }

  /* ── /help modal ─────────────────────────────────────────────────────────── */
  .help-overlay {
    position: fixed; inset: 0; z-index: 250;
    background: rgba(0,0,0,0.45); backdrop-filter: blur(2px);
    display: flex; align-items: center; justify-content: center;
  }
  .help-box {
    background: var(--bg-panel); border: 1px solid var(--border);
    border-radius: 12px; box-shadow: 0 12px 40px rgba(0,0,0,0.4);
    width: 420px; max-width: calc(100vw - 32px);
    overflow: hidden; animation: sm-in 0.14s ease;
  }
  .help-hdr {
    display: flex; align-items: center; justify-content: space-between;
    padding: 13px 16px; border-bottom: 1px solid var(--border);
    font-weight: 700; font-size: 14px; color: var(--text);
  }
  .help-close {
    background: none; color: var(--text-muted); font-size: 14px;
    padding: 3px 6px; border-radius: 5px; line-height: 1;
  }
  .help-close:hover { background: var(--bg-hover); color: var(--text); }
  .help-list { padding: 8px 0; }
  .help-item {
    display: flex; align-items: center; gap: 10px;
    padding: 7px 16px;
    border-bottom: 1px solid var(--border-sub);
  }
  .help-item:last-child { border-bottom: none; }
  .help-icon { font-size: 15px; width: 20px; flex-shrink: 0; text-align: center; }
  .help-cmd { font-size: 12px; font-weight: 700; color: var(--accent); min-width: 80px; }
  .help-desc { font-size: 12px; color: var(--text-muted); flex: 1; }
  .help-hint {
    padding: 8px 16px; font-size: 11px; color: var(--text-dim);
    border-top: 1px solid var(--border); background: var(--bg);
  }
  .help-hint kbd {
    background: var(--bg-hover); border: 1px solid var(--border);
    border-radius: 3px; padding: 0 4px; font-size: 11px;
    font-family: inherit; color: var(--text-muted);
  }

  .send-btn {
    width: 36px; height: 36px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    padding: 0;
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

  /* ── @mention ── */
  :global(mark.mention) {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
    border-radius: 3px;
    padding: 0 2px;
    font-weight: 600;
    font-style: normal;
  }
  :global(mark.mention.mention-mine) {
    background: color-mix(in srgb, #f59e0b 18%, transparent);
    color: #f59e0b;
  }

  .mention-picker {
    border-top: 1px solid var(--border);
    background: var(--bg-panel);
    overflow-y: auto;
    max-height: 160px;
    flex-shrink: 0;
  }
  .mention-item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 7px 14px;
    background: none;
    border-radius: 0;
    font-size: 13px;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
    transition: background 0.1s;
  }
  .mention-item:hover, .mention-item:focus {
    background: var(--bg-hover);
    outline: none;
  }

  /* ── Sticker picker wrapper ───────────────────────────────────────────────── */
  .sticker-picker-wrap {
    position: absolute;
    bottom: 70px;
    left: 8px;
    z-index: 100;
  }

  /* ── Sticker bubble ──────────────────────────────────────────────────────── */
  .sticker-bubble {
    font-size: 72px;
    line-height: 1;
    padding: 4px 0;
    user-select: none;
  }

  /* ── Location bubble ─────────────────────────────────────────────────────── */
  .location-bubble { display: flex; flex-direction: column; gap: 4px; }
  .location-map {
    width: 300px; max-width: 100%;
    height: 150px; object-fit: cover;
    border-radius: 8px;
  }
  .location-footer { font-size: 12px; color: var(--text-muted); }

  /* ── Event bubble ────────────────────────────────────────────────────────── */
  .event-bubble {
    display: flex; flex-direction: column; gap: 4px;
    padding: 2px 0;
    min-width: 200px;
  }
  .event-title { font-weight: 700; font-size: 14px; }
  .event-date { font-size: 12px; color: var(--text-muted); }
  .event-loc { font-size: 12px; color: var(--text-muted); }
  .event-desc { font-size: 12px; color: var(--text); margin-top: 2px; }
  .event-rsvp-btns { display: flex; gap: 6px; margin-top: 6px; }
  .rsvp-btn {
    font-size: 11px; padding: 3px 8px;
    border-radius: 12px;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    color: var(--text);
    cursor: pointer;
    transition: background 0.1s;
  }
  .rsvp-btn:hover { background: var(--accent); color: #fff; border-color: var(--accent); }

  /* ── RSVP bubble ─────────────────────────────────────────────────────────── */
  .rsvp-bubble {
    display: flex; align-items: center; gap: 6px;
    font-size: 13px;
  }
  .rsvp-icon { font-size: 18px; }
  .rsvp-label { font-style: italic; color: var(--text-muted); }

  /* ── Translation ─────────────────────────────────────────────────────────── */
  .translation-text {
    margin-top: 6px;
    padding-top: 6px;
    border-top: 1px solid rgba(128,128,128,0.2);
    font-style: italic;
    font-size: 13px;
    color: var(--text-muted);
  }
  .translate-btn {
    background: none;
    font-size: 13px;
    padding: 2px 4px;
    opacity: 0.6;
    border-radius: 4px;
    line-height: 1;
  }
  .translate-btn:hover { opacity: 1; background: var(--bg-hover); }

  /* ── Thread layout ───────────────────────────────────────────────────────── */
  .pane.thread-open {
    flex-direction: row;
  }
  .pane-main {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
  .thread-btn {
    background: none;
    border: none;
    color: var(--accent, #7c6af7);
    cursor: pointer;
    font-size: 13px;
    padding: 2px 5px;
    border-radius: 4px;
    opacity: 0.7;
    line-height: 1;
  }
  .thread-btn:hover { opacity: 1; background: var(--bg-hover); }
  .thread-count-badge {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--accent, #7c6af7);
    font-size: 12px;
    padding: 2px 6px;
    border-radius: 10px;
    border: 1px solid var(--accent, #7c6af7);
    opacity: 0.8;
    margin-top: 4px;
    align-self: flex-start;
  }
  .thread-count-badge:hover { opacity: 1; background: rgba(124,106,247,0.1); }
</style>
