<script>
  import { invoke } from '@tauri-apps/api/core'

  export let msg
  export let downloadFile
  export let downloadingFiles
  export let getThumbnailUrl
  export let msgKey

  let videoUrl = null
  let loading = false
  let err = false
  let videoEl

  async function load() {
    if (videoUrl || loading || !msg.file_key || !msg.file_id) return
    loading = true
    try {
      const bytes = await invoke('download_file', { fileId: msg.file_id, keyBytes: msg.file_key })
      const blob = new Blob([new Uint8Array(bytes)], { type: msg.file_mime || 'video/webm' })
      videoUrl = URL.createObjectURL(blob)
    } catch { err = true }
    finally { loading = false }
  }

  async function enterPip() {
    if (!videoEl) return
    try {
      if (document.pictureInPictureElement === videoEl) {
        await document.exitPictureInPicture()
      } else {
        await videoEl.requestPictureInPicture()
      }
    } catch {}
  }

  $: thumbUrl = getThumbnailUrl(msgKey, msg.thumb_data)
</script>

<div class="circle-wrap">
  {#if videoUrl}
    <div class="video-wrap">
      <!-- svelte-ignore a11y-media-has-caption -->
      <video class="circle-video" controls src={videoUrl} poster={thumbUrl ?? undefined} bind:this={videoEl}></video>
      {#if 'pictureInPictureEnabled' in document}
        <button class="pip-btn" title="Picture-in-Picture" on:click={enterPip}>⧉</button>
      {/if}
    </div>
  {:else}
    <button class="circle-thumb" on:click={load} disabled={loading || err} aria-label="Play video">
      {#if thumbUrl}
        <img class="circle-img" src={thumbUrl} alt="" />
      {:else}
        <div class="circle-placeholder"></div>
      {/if}
      <span class="circle-play">
        {#if loading}⏳{:else if err}✕{:else}▶{/if}
      </span>
    </button>
  {/if}

  <div class="circle-footer">
    {#if msg.file_size}
      <span class="dur">{Math.round(msg.file_size / 1024)} KB</span>
    {/if}
    {#if msg.file_key}
      <button
        class="dl-btn"
        title="Download"
        disabled={downloadingFiles[msg.file_id]}
        on:click={() => downloadFile(msg)}
      >{downloadingFiles[msg.file_id] ? '…' : '⬇'}</button>
    {/if}
  </div>
</div>

<style>
  .circle-wrap { display: flex; flex-direction: column; gap: 4px; align-items: flex-start; padding: 2px 0; max-width: 100%; }

  .circle-video,
  .circle-thumb,
  .circle-img,
  .circle-placeholder {
    width: 280px;
    max-width: 100%;
    height: 180px;
    border-radius: 8px;
    object-fit: cover;
    display: block;
  }

  .video-wrap { position: relative; display: inline-block; }
  .pip-btn {
    position: absolute;
    top: 6px; right: 6px;
    background: rgba(0,0,0,0.55);
    color: #fff;
    border: none;
    border-radius: 4px;
    font-size: 14px;
    padding: 3px 6px;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
    line-height: 1;
  }
  .video-wrap:hover .pip-btn { opacity: 1; }
  .pip-btn:hover { background: rgba(0,0,0,0.8); }

  .circle-video { background: #000; }

  .circle-thumb {
    position: relative;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
    overflow: hidden;
    flex-shrink: 0;
  }
  .circle-thumb:disabled { cursor: default; }

  .circle-placeholder { background: rgba(0,0,0,0.3); }

  .circle-play {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 36px;
    color: #fff;
    text-shadow: 0 2px 8px rgba(0,0,0,0.7);
    background: rgba(0,0,0,0.18);
    border-radius: 8px;
    transition: background 0.15s;
  }
  .circle-thumb:hover:not(:disabled) .circle-play { background: rgba(0,0,0,0.35); }

  .circle-footer { display: flex; align-items: center; gap: 4px; padding: 0 4px; }
  .dur { font-size: 10px; opacity: 0.6; }
  .dl-btn {
    background: none; color: inherit; font-size: 13px;
    padding: 1px 4px; border-radius: 4px; opacity: 0.7;
  }
  .dl-btn:hover:not(:disabled) { background: rgba(0,0,0,0.12); opacity: 1; }
</style>
