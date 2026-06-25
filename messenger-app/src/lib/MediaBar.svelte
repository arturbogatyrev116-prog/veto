<script>
  import { nowPlaying, stopAudio } from '../stores.js'
  import { onDestroy } from 'svelte'

  let np = null
  let playing = false
  let currentTime = 0
  let duration = 0
  let seeking = false
  let rafId = null

  // Poll current time via requestAnimationFrame for smooth progress
  function tick() {
    if (np?.el && !np.el.paused) {
      currentTime = np.el.currentTime
      duration = np.el.duration || 0
    }
    rafId = requestAnimationFrame(tick)
  }

  const unsub = nowPlaying.subscribe(v => {
    np = v
    if (v) {
      playing = !v.el.paused
      currentTime = v.el.currentTime
      duration = v.el.duration || 0
      // Attach event listeners to the audio element
      v.el.addEventListener('play',  onPlay)
      v.el.addEventListener('pause', onPause)
      v.el.addEventListener('ended', onEnded)
      v.el.addEventListener('loadedmetadata', onMeta)
      if (!rafId) rafId = requestAnimationFrame(tick)
    } else {
      playing = false
      if (rafId) { cancelAnimationFrame(rafId); rafId = null }
    }
  })

  function onPlay()  { playing = true  }
  function onPause() { playing = false }
  function onEnded() { playing = false; currentTime = 0 }
  function onMeta()  { duration = np?.el.duration || 0 }

  function togglePlay() {
    if (!np?.el) return
    if (np.el.paused) np.el.play()
    else              np.el.pause()
  }

  function close() {
    stopAudio()
  }

  function onSeekInput(e) {
    seeking = true
    currentTime = Number(e.target.value)
  }
  function onSeekChange(e) {
    if (np?.el) np.el.currentTime = Number(e.target.value)
    seeking = false
  }

  function fmt(s) {
    if (!isFinite(s) || s < 0) return '0:00'
    const m = Math.floor(s / 60)
    const sec = Math.floor(s % 60)
    return `${m}:${sec.toString().padStart(2, '0')}`
  }

  onDestroy(() => {
    unsub()
    if (rafId) cancelAnimationFrame(rafId)
  })

  $: progress = duration > 0 ? (currentTime / duration) * 100 : 0
</script>

{#if np}
  <div class="media-bar">
    <button class="mb-play" on:click={togglePlay} title={playing ? 'Pause' : 'Play'}>
      {#if playing}
        <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
          <rect x="1" y="1" width="4" height="10" rx="1"/>
          <rect x="7" y="1" width="4" height="10" rx="1"/>
        </svg>
      {:else}
        <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
          <path d="M2 1l9 5-9 5V1z"/>
        </svg>
      {/if}
    </button>

    <div class="mb-info">
      <div class="mb-title">
        <span class="mb-icon">🎙</span>
        <span class="mb-name">{np.peerName || 'Voice message'}</span>
        <span class="mb-time">{fmt(currentTime)} / {fmt(duration)}</span>
      </div>
      <div class="mb-seek">
        <div class="mb-track">
          <div class="mb-fill" style="width:{progress}%"></div>
        </div>
        <input
          type="range"
          class="mb-range"
          min="0"
          max={duration || 0}
          step="0.05"
          value={currentTime}
          on:input={onSeekInput}
          on:change={onSeekChange}
        />
      </div>
    </div>

    <button class="mb-close" on:click={close} title="Stop">×</button>
  </div>
{/if}

<style>
  .media-bar {
    position: fixed;
    top: 0; left: 0; right: 0;
    z-index: 100;
    height: 44px;
    background: var(--bg-panel, #1e1e2e);
    border-bottom: 1px solid var(--accent, #89b4fa);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px;
    box-shadow: 0 2px 12px rgba(0,0,0,0.35);
    animation: slide-in 0.15s ease;
  }
  @keyframes slide-in {
    from { transform: translateY(-100%); opacity: 0; }
    to   { transform: translateY(0);     opacity: 1; }
  }

  .mb-play {
    width: 28px; height: 28px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff; border: none; cursor: pointer;
    display: flex; align-items: center; justify-content: center;
    flex-shrink: 0;
    transition: opacity 0.12s;
  }
  .mb-play:hover { opacity: 0.85; }

  .mb-info {
    flex: 1; min-width: 0;
    display: flex; flex-direction: column; gap: 2px;
  }

  .mb-title {
    display: flex; align-items: center; gap: 6px;
    font-size: 11px;
  }
  .mb-icon { font-size: 12px; }
  .mb-name { font-weight: 600; color: var(--text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .mb-time { font-size: 10px; color: var(--text-muted); margin-left: auto; flex-shrink: 0; font-variant-numeric: tabular-nums; }

  .mb-seek {
    position: relative; height: 12px;
    display: flex; align-items: center;
  }
  .mb-track {
    width: 100%; height: 3px;
    background: rgba(128,128,128,0.3);
    border-radius: 2px; overflow: hidden;
    pointer-events: none;
  }
  .mb-fill {
    height: 100%; background: var(--accent);
    border-radius: 2px; transition: width 0.08s linear;
  }
  .mb-range {
    position: absolute; inset: 0;
    width: 100%; height: 100%;
    opacity: 0; cursor: pointer;
    margin: 0; padding: 0;
    -webkit-appearance: none;
  }

  .mb-close {
    width: 24px; height: 24px;
    background: none; border: none;
    color: var(--text-muted); font-size: 18px; line-height: 1;
    cursor: pointer; border-radius: 50%; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    transition: background 0.1s;
  }
  .mb-close:hover { background: var(--bg-hover); color: var(--text); }
</style>
