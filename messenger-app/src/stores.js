import { writable } from 'svelte/store'

// { user_id: string, username: string } | null
export const user = writable(null)

// { [peer_id]: [{ from: string, text: string, ts: number }] }
export const conversations = writable({})

// peer_id | null
export const activeConv = writable(null)

// true when WS connection is lost
export const connLost = writable(false)

// true after successful unlock (Argon2id key derived and sessions loaded)
export const unlocked = writable(false)

// { [peer_id]: true } — peers currently typing; entries cleared after 4 s of silence
export const typingPeers = writable({})


// 'connecting' | 'connected' | 'lost'
export const wsStatus = writable('connecting')

// { [peer_id]: displayName } — cached names so sidebar shows username not UUID
export const peerNames = writable({})

export function addMessage(peerId, msg) {
  conversations.update(convs => {
    const msgs = convs[peerId] ?? []
    return { ...convs, [peerId]: [...msgs, msg] }
  })
}

export function setPeerName(peerId, name) {
  peerNames.update(n => ({ ...n, [peerId]: name }))
}

// Set of user_ids currently online (updated via presence/hello events)
export const onlinePeers = writable(new Set())

// { [group_id]: GroupInfo } — populated by load_groups / create_group
export const groups = writable({})

export function removeMessage(peerId, ts, from) {
  conversations.update(convs => {
    const msgs = (convs[peerId] ?? []).filter(m => !(m.ts === ts && m.from === from))
    return { ...convs, [peerId]: msgs }
  })
}

export function addGroupMessage(groupId, msg) {
  conversations.update(convs => {
    const msgs = convs[groupId] ?? []
    return { ...convs, [groupId]: [...msgs, msg] }
  })
}

// true when the full-text search modal is open
export const showSearch = writable(false)

// { [peer_id]: number } — unread received message count per conversation
export const unreadCounts = writable({})

// { [peer_id]: { [msgKey]: { [emoji]: [reactorId] } } }
// msgKey = `${msg_ts}_${msg_from}`
export const reactions = writable({})

// { [peer_id]: boolean } — true when chat is muted; polled lazily by Sidebar
export const mutedConvs = writable({})

// Currently playing media: { el: HTMLAudioElement, title: string, peerName: string, ts: number } | null
// Only one audio plays at a time; setting a new one stops the previous.
export const nowPlaying = writable(null)

export function playAudio(el, meta) {
  // Stop whatever is playing now
  let prev
  nowPlaying.subscribe(v => { prev = v })()
  if (prev && prev.el !== el) {
    prev.el.pause()
  }
  nowPlaying.set({ el, ...meta })
}

export function stopAudio() {
  let prev
  nowPlaying.subscribe(v => { prev = v })()
  if (prev) { prev.el.pause() }
  nowPlaying.set(null)
}
