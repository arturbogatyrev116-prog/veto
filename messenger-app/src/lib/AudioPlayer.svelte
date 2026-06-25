<script>
  import { invoke } from '@tauri-apps/api/core'
  import { onDestroy } from 'svelte'
  import { nowPlaying, playAudio, stopAudio } from '../stores.js'

  export let msg
  export let downloadFile
  export let downloadingFiles
  export let peerName = ''

  let audioUrl = null
  let loading = false
  let err = false
  let audioEl

  let playing = false
  let currentTime = 0
  let duration = 0
  let speed = 1
  let seeking = false

  const SPEEDS = [1, 1.5, 2]

  async function loadAndPlay() {
    if (err) return
    if (!audioUrl) {
      if (loading) return
      loading = true
      try {
        const bytes = await invoke('download_file', { fileId: msg.file_id, keyBytes: msg.file_key })
        const blob = new Blob([new Uint8Array(bytes)], { type: msg.file_mime || 'audio/webm' })
        audioUrl = URL.createObjectURL(blob)
      } catch { err = true; loading = false; return }
      loading = false
    }
    // Wait one tick for <audio> to mount
    await new Promise(r => setTimeout(r, 0))
    togglePlay()
  }

  function togglePlay() {
    if (!audioEl) return
    if (audioEl.paused) {
      playAudio(audioEl, { title: 'Voice message', peerName, ts: msg.ts })
      audioEl.play()
    } else {
      audioEl.pause()
    }
  }

  function cycleSpeed() {
    const idx = SPEEDS.indexOf(speed)
    speed = SPEEDS[(idx + 1) % SPEEDS.length]
    if (audioEl) audioEl.playbackRate = speed
  }

  function onTimeUpdate() {
    if (!seeking) currentTime = audioEl.currentTime
  }

  function onLoadedMetadata() {
    duration = audioEl.duration
    audioEl.playbackRate = speed
  }

  function onEnded() {
    playing = false
    currentTime = 0
    nowPlaying.update(np => np?.el === audioEl ? null : np)
  }

  function onSeekInput(e) {
    seeking = true
    currentTime = Number(e.target.value)
  }

  function onSeekChange(e) {
    if (audioEl) audioEl.currentTime = Number(e.target.value)
    seeking = false
  }

  // Sync playing state from audio element events
  function onPlay()  { playing = true  }
  function onPause() { playing = false }

  function fmt(s) {
    if (!isFinite(s)) return '0:00'
    const m = Math.floor(s / 60)
    const sec = Math.floor(s % 60)
    return `${m}:${sec.toString().padStart(2, '0')}`
  }

  // When another audio starts, this one pauses automatically (handled by playAudio store)
  const unsubNp = nowPlaying.subscribe(np => {
    if (audioEl && np?.el !== audioEl && !audioEl.paused) {
      audioEl.pause()
    }
  })
  onDestroy(() => {
    unsubNp()
    if (audioEl && !audioEl.paused) audioEl.pause()
    if (audioUrl) URL.revokeObjectURL(audioUrl)
  })
</script>

<div class="audio-msg">
  <!-- Play/pause button -->
  <button
    class="play-btn"
    class:loading
    disabled={loading || err}
    on:click={audioUrl ? togglePlay : loadAndPlay}
    title={playing ? 'Pause' : 'Play'}
  >
    {#if loading}
      <span class="spin">⏳</span>
    {:else if err}
      <span>✕</span>
    {:else if playing}
      <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor">
        <rect x="2" y="1" width="4" height="12" rx="1"/>
        <rect x="8" y="1" width="4" height="12" rx="1"/>
      </svg>
    {:else}
      <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor">
        <path d="M3 1.5l9 5.5-9 5.5V1.5z"/>
      </svg>
    {/if}
  </button>

  <!-- Progress + time -->
  <div class="progress-area">
    <div class="time-row">
      <span class="time">{fmt(currentTime)}</span>
      <span class="time-sep">/</span>
      <span class="time dur">{fmt(duration)}</span>
      <button class="speed-btn" on:click={cycleSpeed} title="Playback speed">{speed}×</button>
    </div>
    <div class="seek-wrap">
      <div class="seek-track">
        <div
          class="seek-fill"
          style="width: {duration > 0 ? (currentTime / duration) * 100 : 0}%"
        ></div>
      </div>
      {#if audioUrl}
        <input
          type="range"
          class="seek-input"
          min="0"
          max={duration || 0}
          step="0.05"
          value={currentTime}
          on:input={onSeekInput}
          on:change={onSeekChange}
        />
      {/if}
    </div>
  </div>

  <!-- Download -->
  {#if msg.file_key}
    <button
      class="dl-btn"
      title="Download"
      disabled={downloadingFiles[msg.file_id]}
      on:click={() => downloadFile(msg)}
    >{downloadingFiles[msg.file_id] ? '…' : '⬇'}</button>
  {/if}
</div>

<!-- Hidden audio element, mounted only after URL is ready -->
{#if audioUrl}
  <!-- svelte-ignore a11y-media-has-caption -->
  <audio
    src={audioUrl}
    bind:this={audioEl}
    on:timeupdate={onTimeUpdate}
    on:loadedmetadata={onLoadedMetadata}
    on:ended={onEnded}
    on:play={onPlay}
    on:pause={onPause}
    style="display:none"
  ></audio>
{/if}

<style>
  .audio-msg {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    min-width: 200px;
    max-width: 280px;
  }

  .play-btn {
    width: 34px; height: 34px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    border: none;
    cursor: pointer;
    display: flex; align-items: center; justify-content: center;
    flex-shrink: 0;
    transition: opacity 0.15s, transform 0.1s;
  }
  .play-btn:hover:not(:disabled) { opacity: 0.88; transform: scale(1.04); }
  .play-btn:disabled { background: var(--bg-hover); color: var(--text-muted); cursor: default; }
  .play-btn.loading { background: var(--bg-hover); }
  .spin { animation: spin 1s linear infinite; display: inline-block; }
  @keyframes spin { to { transform: rotate(360deg); } }

  .progress-area {
    flex: 1;
    display: flex; flex-direction: column; gap: 2px;
    min-width: 0;
  }

  .time-row {
    display: flex; align-items: center; gap: 3px;
    font-size: 10px; color: var(--text-muted);
  }
  .time { font-variant-numeric: tabular-nums; }
  .time-sep { opacity: 0.5; }
  .dur { opacity: 0.6; }
  .speed-btn {
    margin-left: auto;
    background: var(--bg-hover);
    border: none; border-radius: 3px;
    color: var(--accent); font-size: 10px; font-weight: 700;
    padding: 1px 4px; cursor: pointer;
    transition: background 0.1s;
  }
  .speed-btn:hover { background: var(--bg-active); }

  .seek-wrap {
    position: relative; height: 16px;
    display: flex; align-items: center;
  }
  .seek-track {
    width: 100%; height: 4px;
    background: rgba(128,128,128,0.25);
    border-radius: 2px;
    overflow: hidden;
    pointer-events: none;
  }
  .seek-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.1s linear;
  }
  .seek-input {
    position: absolute; inset: 0;
    width: 100%; height: 100%;
    opacity: 0;
    cursor: pointer;
    margin: 0; padding: 0;
    -webkit-appearance: none;
  }

  .dl-btn {
    background: none; color: var(--text-muted); font-size: 14px;
    padding: 3px 5px; border-radius: 4px; flex-shrink: 0;
    opacity: 0.7;
  }
  .dl-btn:hover:not(:disabled) { background: var(--bg-hover); opacity: 1; }
</style>
