<script>
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { tick, onMount, onDestroy } from 'svelte'
  import { user, peerNames } from '../stores.js'
  import Avatar from './Avatar.svelte'

  export let groupId    // group UUID
  export let parentMsg  // { ts, from, text, from_name }
  export let onClose = null

  let messages = []
  let text = ''
  let sending = false
  let threadEl

  $: parentFromName = $peerNames[parentMsg?.from] ?? (parentMsg?.from ?? '').slice(0, 8)

  async function load() {
    if (!parentMsg) return
    try {
      const decrypted = await invoke('get_thread_messages', {
        groupId,
        parentTs: parentMsg.ts,
      })
      messages = decrypted
      await tick()
      scrollBottom()
    } catch (e) {
      console.warn('get_thread_messages failed:', e)
    }
  }

  $: if (parentMsg?.ts) load()

  function scrollBottom() {
    if (threadEl) threadEl.scrollTop = threadEl.scrollHeight
  }

  async function send() {
    const msg = text.trim()
    if (!msg || sending) return
    sending = true
    try {
      await invoke('send_group_message', {
        groupId,
        text: msg,
        replyTo: null,
        mentions: null,
        payloadOverride: null,
        threadParentTs: parentMsg.ts,
        threadParentFrom: parentMsg.from,
      })
      text = ''
      await load()
    } catch (e) {
      console.warn('send thread reply failed:', e)
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

  let unlisten
  onMount(async () => {
    unlisten = await listen('group_message', ({ payload }) => {
      if (payload.gid !== groupId) return
      if (payload.thread_parent_ts !== parentMsg?.ts) return
      messages = [...messages, {
        from: payload.from,
        sender_id: payload.from,
        text: payload.text,
        ts: payload.ts ?? Date.now(),
        status: 'delivered',
        group_id: groupId,
        thread_parent_ts: payload.thread_parent_ts,
        thread_parent_from: payload.thread_parent_from,
        thread_reply_count: 0,
        file_id: null, file_key: null, file_name: null, file_mime: null, file_size: null,
        reply_to_ts: null, reply_to_from: null, reply_to_text: null,
      }]
      tick().then(scrollBottom)
    })
  })
  onDestroy(() => unlisten?.())

  function fmtTime(ts) {
    const d = new Date(ts)
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }
</script>

<div class="thread-pane" role="complementary" aria-label="Thread">
  <div class="thread-header">
    <span class="thread-title">Thread</span>
    <button class="thread-close" on:click={onClose} aria-label="Close thread">✕</button>
  </div>

  <!-- Parent message preview -->
  <div class="thread-parent">
    <Avatar name={parentFromName} uid={parentMsg?.from} size={24} />
    <div class="thread-parent-body">
      <span class="thread-parent-name">{parentFromName}</span>
      <span class="thread-parent-text">{parentMsg?.text?.length > 120 ? parentMsg.text.slice(0, 117) + '…' : (parentMsg?.text ?? '')}</span>
    </div>
  </div>

  <div class="thread-divider">{messages.length} {messages.length === 1 ? 'reply' : 'replies'}</div>

  <!-- Thread replies -->
  <div class="thread-messages" bind:this={threadEl}>
    {#each messages as m (m.ts + '_' + (m.sender_id ?? m.from))}
      {@const myId = $user?.user_id}
      {@const mine = (m.sender_id ?? m.from) === myId}
      {@const fromName = $peerNames[m.sender_id ?? m.from] ?? (m.sender_id ?? m.from ?? '').slice(0, 8)}
      <div class="thread-msg" class:mine>
        {#if !mine}
          <Avatar name={fromName} uid={m.sender_id ?? m.from} size={22} />
        {/if}
        <div class="thread-msg-body">
          {#if !mine}
            <span class="thread-msg-name">{fromName}</span>
          {/if}
          <div class="thread-bubble">{m.text}</div>
          <span class="thread-msg-time">{fmtTime(m.ts)}</span>
        </div>
      </div>
    {/each}
    {#if messages.length === 0}
      <div class="thread-empty">No replies yet. Be the first!</div>
    {/if}
  </div>

  <!-- Compose bar -->
  <div class="thread-compose">
    <textarea
      class="thread-input"
      placeholder="Reply in thread…"
      rows="2"
      bind:value={text}
      disabled={sending}
      on:keydown={onKeydown}
    />
    <button
      class="thread-send"
      disabled={sending || !text.trim()}
      on:click={send}
      aria-label="Send reply"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
    </button>
  </div>
</div>

<style>
  .thread-pane {
    display: flex;
    flex-direction: column;
    width: 320px;
    min-width: 260px;
    border-left: 1px solid var(--border, #333);
    background: var(--bg, #1a1a2e);
    height: 100%;
    overflow: hidden;
  }
  .thread-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border, #333);
    flex-shrink: 0;
  }
  .thread-title {
    font-weight: 600;
    font-size: 14px;
    color: var(--text, #e0e0e0);
  }
  .thread-close {
    background: none;
    border: none;
    color: var(--text-dim, #888);
    cursor: pointer;
    font-size: 16px;
    padding: 2px 6px;
    border-radius: 4px;
    line-height: 1;
  }
  .thread-close:hover { background: var(--hover, #2a2a3e); color: var(--text, #e0e0e0); }

  .thread-parent {
    display: flex;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg-raised, #22223a);
    border-bottom: 1px solid var(--border, #333);
    flex-shrink: 0;
  }
  .thread-parent-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .thread-parent-name {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent, #7c6af7);
  }
  .thread-parent-text {
    font-size: 12px;
    color: var(--text-dim, #888);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .thread-divider {
    text-align: center;
    font-size: 11px;
    color: var(--text-dim, #888);
    padding: 6px 0;
    border-bottom: 1px solid var(--border, #333);
    flex-shrink: 0;
  }

  .thread-messages {
    flex: 1;
    overflow-y: auto;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .thread-empty {
    color: var(--text-dim, #888);
    font-size: 13px;
    text-align: center;
    margin-top: 24px;
  }

  .thread-msg {
    display: flex;
    gap: 6px;
    align-items: flex-start;
  }
  .thread-msg.mine {
    flex-direction: row-reverse;
  }
  .thread-msg-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: 220px;
  }
  .mine .thread-msg-body { align-items: flex-end; }
  .thread-msg-name {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent, #7c6af7);
  }
  .thread-bubble {
    background: var(--bubble-in, #2a2a4a);
    color: var(--text, #e0e0e0);
    border-radius: 10px;
    padding: 6px 10px;
    font-size: 13px;
    word-break: break-word;
    white-space: pre-wrap;
  }
  .mine .thread-bubble {
    background: var(--bubble-out, #4a3fa0);
    color: #fff;
  }
  .thread-msg-time {
    font-size: 10px;
    color: var(--text-dim, #888);
  }

  .thread-compose {
    display: flex;
    gap: 6px;
    padding: 8px 10px;
    border-top: 1px solid var(--border, #333);
    align-items: flex-end;
    flex-shrink: 0;
  }
  .thread-input {
    flex: 1;
    background: var(--input-bg, #252540);
    border: 1px solid var(--border, #333);
    border-radius: 8px;
    color: var(--text, #e0e0e0);
    font-size: 13px;
    padding: 6px 10px;
    resize: none;
    line-height: 1.4;
    font-family: inherit;
  }
  .thread-input:focus { outline: none; border-color: var(--accent, #7c6af7); }
  .thread-send {
    background: var(--accent, #7c6af7);
    border: none;
    border-radius: 8px;
    color: #fff;
    cursor: pointer;
    padding: 8px 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .thread-send:disabled { opacity: 0.5; cursor: not-allowed; }
  .thread-send:not(:disabled):hover { background: var(--accent-hover, #6a59e0); }
</style>
