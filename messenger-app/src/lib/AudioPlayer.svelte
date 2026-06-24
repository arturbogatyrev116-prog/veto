<script>
  import { invoke } from '@tauri-apps/api/core'

  export let msg
  export let downloadFile
  export let downloadingFiles

  let audioUrl = null
  let loading = false
  let err = false

  async function load() {
    if (audioUrl || loading || !msg.file_key || !msg.file_id) return
    loading = true
    try {
      const bytes = await invoke('download_file', { fileId: msg.file_id, keyBytes: msg.file_key })
      const blob = new Blob([new Uint8Array(bytes)], { type: msg.file_mime || 'audio/webm' })
      audioUrl = URL.createObjectURL(blob)
    } catch { err = true }
    finally { loading = false }
  }
</script>

<div class="audio-msg">
  {#if audioUrl}
    <!-- svelte-ignore a11y-media-has-caption -->
    <audio class="audio-el" controls src={audioUrl}></audio>
  {:else if err}
    <span class="audio-err">Failed to load</span>
  {:else}
    <button class="audio-load-btn" on:click={load} disabled={loading}>
      {loading ? '…' : '▶ Voice message'}
    </button>
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

<style>
  .audio-msg { display: flex; align-items: center; gap: 6px; padding: 2px 0 4px; }
  .audio-el { height: 32px; min-width: 180px; max-width: 260px; }
  .audio-load-btn {
    font-size: 12px; padding: 5px 12px;
    background: rgba(0,0,0,0.15); color: inherit;
    border: 1px solid rgba(255,255,255,0.15); border-radius: 999px;
    cursor: pointer; white-space: nowrap;
  }
  .audio-load-btn:disabled { opacity: 0.6; cursor: default; }
  .audio-load-btn:hover:not(:disabled) { background: rgba(0,0,0,0.25); }
  .audio-err { font-size: 11px; opacity: 0.6; }
  .file-dl-btn {
    background: none; color: inherit; font-size: 14px;
    padding: 2px 5px; border-radius: 4px; opacity: 0.7;
    flex-shrink: 0;
  }
  .file-dl-btn:hover:not(:disabled) { background: rgba(0,0,0,0.12); opacity: 1; }
</style>
