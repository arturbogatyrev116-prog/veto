<script>
  import { invoke } from '@tauri-apps/api/core'
  import { unlocked } from '../stores.js'

  let password = ''
  let error = ''
  let loading = false
  let isFirstTime = false
  let resetting = false

  async function resetIdentity() {
    if (!confirm('Delete all local data and start over? This cannot be undone.')) return
    resetting = true
    try {
      await invoke('clear_identity')
      // Reload to show Register screen
      window.location.reload()
    } catch (e) {
      error = 'Reset failed: ' + e
      resetting = false
    }
  }

  async function handleSubmit() {
    if (!password) return
    loading = true
    error = ''
    try {
      const restored = await invoke('unlock', { password })
      isFirstTime = !restored
      // App.svelte watches $unlocked and handles connect() + retry loop.
      unlocked.set(true)
    } catch (e) {
      const msg = String(e)
      error = msg === 'incorrect_password'
        ? 'Incorrect password. Try again.'
        : msg
    } finally {
      loading = false
    }
  }
</script>

<div class="wrap">
  <div class="card">
    <div class="icon">🔐</div>
    <h2>Session Password</h2>
    <p class="hint">
      Your encryption keys are protected by this password.<br>
      It never leaves your device.
    </p>

    <form on:submit|preventDefault={handleSubmit}>
      <input
        type="password"
        bind:value={password}
        placeholder="Password"
        disabled={loading}
        autocomplete="current-password"
      />
      <button type="submit" disabled={loading || !password}>
        {#if loading}
          <span class="spinner">⟳</span> Unlocking…
        {:else}
          Unlock
        {/if}
      </button>
    </form>

    {#if error}
      <p class="err">{error}</p>
    {/if}

    <button class="reset-btn" on:click={resetIdentity} disabled={loading || resetting}>
      {resetting ? 'Resetting…' : 'Delete local data & start over'}
    </button>
  </div>
</div>

<style>
  .wrap {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    background: var(--bg);
  }

  .card {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 40px 36px;
    width: 340px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
  }

  .icon { font-size: 32px; text-align: center; }

  h2 {
    font-size: 20px;
    font-weight: 700;
    text-align: center;
    color: var(--text);
    margin: 0;
  }

  .hint {
    font-size: 13px;
    color: var(--text-muted);
    text-align: center;
    line-height: 1.5;
    margin: 0;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 4px;
  }

  input { width: 100%; font-size: 15px; padding: 10px 14px; }

  button {
    padding: 11px;
    font-size: 15px;
    font-weight: 600;
    background: var(--accent);
    color: #fff;
    border-radius: var(--radius);
    letter-spacing: 0.01em;
  }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button:not(:disabled):hover { filter: brightness(1.12); }

  .spinner {
    display: inline-block;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  .err {
    color: var(--danger);
    font-size: 13px;
    text-align: center;
    margin: 0;
  }

  .reset-btn {
    background: none;
    color: var(--text-dim, #6b7280);
    font-size: 12px;
    font-weight: 400;
    border: none;
    padding: 4px;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
    letter-spacing: 0;
  }
  .reset-btn:hover { color: var(--danger); }
  .reset-btn:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
