<script>
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { user } from '../stores.js'

  let username = ''
  let error = ''
  let loading = false

  // Server URL configuration
  let serverUrl = 'http://localhost:3000'
  let showServerEdit = false
  let serverUrlInput = ''
  let serverSaving = false
  let serverSaveErr = ''
  let serverSaved = false

  onMount(async () => {
    try { serverUrl = await invoke('get_server_url') } catch {}
  })

  async function handleRegister() {
    const name = username.trim()
    if (!name) return
    loading = true
    error = ''
    try {
      const info = await invoke('register', { username: name })
      user.set(info)
    } catch (e) {
      error = String(e)
    } finally {
      loading = false
    }
  }

  function openServerEdit() {
    serverUrlInput = serverUrl
    serverSaveErr = ''
    showServerEdit = true
  }

  async function saveServerUrl() {
    const url = serverUrlInput.trim()
    if (!url) return
    serverSaving = true
    serverSaveErr = ''
    serverSaved = false
    try {
      await invoke('set_server_url', { url })
      serverUrl = url
      serverSaved = true
      setTimeout(() => { showServerEdit = false; serverSaved = false }, 2000)
    } catch (e) {
      serverSaveErr = String(e)
    } finally {
      serverSaving = false
    }
  }
</script>

<div class="register-wrap">
  <div class="register-card">
    <h1>Veto</h1>
    <p>Choose a username to get started.</p>
    <form on:submit|preventDefault={handleRegister}>
      <input
        type="text"
        bind:value={username}
        placeholder="Username"
        maxlength="64"
        disabled={loading}
      />
      <button type="submit" disabled={loading || !username.trim()}>
        {loading ? 'Registering…' : 'Register'}
      </button>
    </form>
    {#if error}
      <p class="err">{error}</p>
    {/if}

    <div class="server-section">
      {#if !showServerEdit}
        <button class="server-link" on:click={openServerEdit}>
          Server: {serverUrl}
        </button>
      {:else}
        <div class="server-edit">
          <input
            type="text"
            bind:value={serverUrlInput}
            placeholder="http://100.x.x.x:3000"
            on:keydown={(e) => e.key === 'Enter' && saveServerUrl()}
          />
          <div class="server-btns">
            <button class="btn-save" on:click={saveServerUrl} disabled={serverSaving}>
              {serverSaving ? 'Saving…' : serverSaved ? 'Saved!' : 'Save'}
            </button>
            <button class="btn-cancel" on:click={() => showServerEdit = false}>Cancel</button>
          </div>
          {#if serverSaved}
            <p class="saved-note">Restart the app to connect to the new server.</p>
          {:else if serverSaveErr}
            <p class="err">{serverSaveErr}</p>
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .register-wrap {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
  }
  .register-card {
    background: #111827;
    border: 1px solid #374151;
    border-radius: 12px;
    padding: 40px;
    width: 340px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  h1 { font-size: 24px; }
  p { color: #9ca3af; }
  form { display: flex; flex-direction: column; gap: 10px; }
  input { width: 100%; }
  .err { color: #f87171; font-size: 13px; }

  .server-section { margin-top: 4px; border-top: 1px solid #374151; padding-top: 12px; }

  .server-link {
    background: none;
    border: none;
    color: #6b7280;
    font-size: 11px;
    cursor: pointer;
    padding: 0;
    text-align: left;
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .server-link:hover { color: #9ca3af; text-decoration: underline; }

  .server-edit { display: flex; flex-direction: column; gap: 8px; }
  .server-edit input { font-size: 12px; }
  .server-btns { display: flex; gap: 8px; }

  .btn-save {
    flex: 1;
    padding: 6px 10px;
    background: #4f46e5;
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }
  .btn-save:hover { background: #4338ca; }
  .btn-save:disabled { opacity: 0.6; cursor: default; }

  .btn-cancel {
    padding: 6px 10px;
    background: transparent;
    color: #6b7280;
    border: 1px solid #374151;
    border-radius: 6px;
    font-size: 12px;
    cursor: pointer;
  }
  .btn-cancel:hover { color: #9ca3af; }
  .saved-note { color: #34d399; font-size: 12px; }
</style>
