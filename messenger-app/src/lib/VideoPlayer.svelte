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

  $: thumbUrl = getThumbnailUrl(msgKey, msg.thumb_data)
</script>

<div class="video-msg">
  {#if videoUrl}
    <!-- svelte-ignore a11y-media-has-caption -->
    <video class="video-el" controls src={videoUrl} poster={thumbUrl ?? undefined}></video>
  {:else}
    <button class="video-thumb-btn" on:click={load} disabled={loading || err}>
      {#if thumbUrl}
        <img class="video-thumb" src={thumbUrl} alt="video" />
        <span class="video-play-icon">{loading ? '…' : err ? '✕' : '▶'}</span>
      {:else}
        <span class="video-no-thumb">{loading ? '…' : err ? 'Failed' : '▶ Video message'}</span>
      {/if}
    </button>
  {/if}
  <div class="video-footer">
    {#if msg.file_size}
      <span class="file-size">{Math.round(msg.file_size / 1024)} KB</span>
    {/if}
    {#if msg.file_key}
      <button
        class="file-dl-btn"
        title="Download"
        disabled={downloadingFiles[msg.file_id]}
        on:click={() => downloadFile(msg)}
      >{downloadingFiles[msg.file_id] ? '…' : '⬇'}</button>
    {/if}
  </div>
</div>

<style>
  .video-msg { display: flex; flex-direction: column; gap: 4px; max-width: 280px; padding: 2px 0 4px; }
  .video-el {
    width: 100%; max-width: 280px; border-radius: 8px;
    background: #000; display: block;
  }
  .video-thumb-btn {
    position: relative; background: none; padding: 0;
    border-radius: 8px; overflow: hidden; cursor: pointer;
    display: block; width: 100%;
  }
  .video-thumb {
    width: 100%; max-width: 280px; border-radius: 8px;
    display: block; object-fit: cover; aspect-ratio: 4/3;
    background: #111;
  }
  .video-play-icon {
    position: absolute; inset: 0;
    display: flex; align-items: center; justify-content: center;
    font-size: 32px; color: #fff;
    text-shadow: 0 2px 8px rgba(0,0,0,0.7);
    background: rgba(0,0,0,0.2);
  }
  .video-no-thumb {
    display: flex; align-items: center; justify-content: center;
    width: 200px; height: 120px; border-radius: 8px;
    background: rgba(0,0,0,0.2); font-size: 13px; color: inherit;
  }
  .video-footer { display: flex; align-items: center; gap: 4px; }
  .file-size { font-size: 10px; opacity: 0.6; flex: 1; }
  .file-dl-btn {
    background: none; color: inherit; font-size: 14px;
    padding: 2px 5px; border-radius: 4px; opacity: 0.7; flex-shrink: 0;
  }
  .file-dl-btn:hover:not(:disabled) { background: rgba(0,0,0,0.12); opacity: 1; }
</style>
