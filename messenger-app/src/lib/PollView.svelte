<script>
  import { invoke } from '@tauri-apps/api/core'
  import { user } from '../stores.js'
  import { onMount } from 'svelte'

  export let pollId
  export let mine = false

  let poll = null
  let loading = true
  let voting = false
  let err = ''

  onMount(async () => {
    await loadPoll()
  })

  async function loadPoll() {
    loading = true; err = ''
    try {
      poll = await invoke('get_poll', { pollId })
    } catch (e) {
      err = String(e)
    }
    loading = false
  }

  async function vote(optionId) {
    if (voting || poll?.closed) return
    voting = true; err = ''
    try {
      poll = await invoke('vote_poll', { pollId, optionId })
    } catch (e) {
      err = String(e)
    }
    voting = false
  }

  async function closePoll() {
    if (!confirm('Close this poll? No more votes will be accepted.')) return
    try {
      poll = await invoke('close_poll', { pollId })
    } catch (e) {
      err = String(e)
    }
  }

  $: myId = $user?.user_id
  $: isCreator = poll?.creator_id === myId
  $: maxVotes = poll ? Math.max(1, ...poll.options.map(o => o.votes)) : 1
</script>

<div class="poll-card">
  {#if loading}
    <div class="poll-loading">Loading poll…</div>
  {:else if err}
    <div class="poll-err">{err} <button on:click={loadPoll}>retry</button></div>
  {:else if poll}
    <div class="poll-header">
      <span class="poll-icon">📊</span>
      <span class="poll-question">{poll.question}</span>
      {#if poll.closed}<span class="poll-closed-badge">Closed</span>{/if}
    </div>

    <div class="poll-options">
      {#each poll.options as opt}
        {@const pct = poll.total_votes > 0 ? Math.round(opt.votes / poll.total_votes * 100) : 0}
        {@const voted = poll.my_vote === opt.id}
        <button
          class="poll-opt"
          class:voted
          class:closed={poll.closed}
          on:click={() => vote(opt.id)}
          disabled={poll.closed || voting}
        >
          <div class="poll-bar" style="width:{pct}%"></div>
          <span class="poll-opt-text">{opt.text}</span>
          <span class="poll-opt-meta">
            {#if voted}<span class="checkmark">✓</span>{/if}
            {pct}% · {opt.votes}
          </span>
        </button>
      {/each}
    </div>

    <div class="poll-footer">
      <span class="poll-total">{poll.total_votes} vote{poll.total_votes !== 1 ? 's' : ''}</span>
      <button class="poll-refresh" on:click={loadPoll} title="Refresh">↻</button>
      {#if isCreator && !poll.closed}
        <button class="poll-close-btn" on:click={closePoll}>Close poll</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .poll-card {
    background: var(--bg-base, #1e1e2e);
    border: 1px solid var(--border, #313244);
    border-radius: 10px;
    padding: 12px 14px;
    min-width: 220px;
    max-width: 320px;
  }

  .poll-loading, .poll-err {
    font-size: 12px;
    color: var(--text-muted);
    padding: 6px 0;
  }
  .poll-err button {
    background: none; border: none; color: var(--accent); cursor: pointer; font-size: 12px;
  }

  .poll-header {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin-bottom: 10px;
  }
  .poll-icon { font-size: 14px; flex-shrink: 0; margin-top: 1px; }
  .poll-question {
    font-weight: 600;
    font-size: 13px;
    color: var(--text);
    flex: 1;
    line-height: 1.3;
  }
  .poll-closed-badge {
    font-size: 10px;
    background: rgba(128,128,128,0.2);
    color: var(--text-muted);
    border-radius: 4px;
    padding: 2px 6px;
    flex-shrink: 0;
    align-self: center;
  }

  .poll-options { display: flex; flex-direction: column; gap: 6px; }

  .poll-opt {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 10px;
    border: 1px solid var(--border, #313244);
    border-radius: 7px;
    background: var(--bg-panel, #181825);
    cursor: pointer;
    overflow: hidden;
    text-align: left;
    transition: border-color 0.12s;
  }
  .poll-opt:not(:disabled):hover { border-color: var(--accent); }
  .poll-opt.voted { border-color: var(--accent); }
  .poll-opt.closed { cursor: default; }
  .poll-opt:disabled { opacity: 0.8; }

  .poll-bar {
    position: absolute;
    inset: 0 auto 0 0;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    border-radius: 7px;
    pointer-events: none;
    transition: width 0.3s ease;
    min-width: 0;
  }
  .poll-opt.voted .poll-bar {
    background: color-mix(in srgb, var(--accent) 28%, transparent);
  }

  .poll-opt-text {
    position: relative;
    flex: 1;
    font-size: 12px;
    color: var(--text);
    z-index: 1;
  }
  .poll-opt-meta {
    position: relative;
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    z-index: 1;
  }
  .checkmark { color: var(--accent); font-size: 11px; margin-right: 3px; }

  .poll-footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }
  .poll-total { font-size: 11px; color: var(--text-muted); flex: 1; }
  .poll-refresh {
    background: none; border: none; color: var(--text-muted); font-size: 14px;
    cursor: pointer; padding: 2px 4px; border-radius: 4px;
    transition: color 0.1s;
  }
  .poll-refresh:hover { color: var(--accent); }
  .poll-close-btn {
    font-size: 11px; color: var(--text-muted);
    background: none; border: 1px solid var(--border);
    border-radius: 5px; padding: 3px 8px; cursor: pointer;
    transition: border-color 0.1s, color 0.1s;
  }
  .poll-close-btn:hover { border-color: #e05555; color: #e05555; }
</style>
