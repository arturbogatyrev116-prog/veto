<script>
  import { invoke } from '@tauri-apps/api/core'
  import { unlocked } from '../stores.js'
  import { onMount } from 'svelte'

  let password = ''
  let error = ''
  let loading = false
  let isFirstTime = false
  let resetting = false
  let hasBiometric = false
  let biometricLoading = false
  let justUnlocked = false
  let offerBiometric = false

  onMount(async () => {
    hasBiometric = await invoke('has_biometric_unlock').catch(() => false)
    if (hasBiometric) tryBiometric()
  })

  async function tryBiometric() {
    biometricLoading = true; error = ''
    try {
      const ok = await invoke('try_biometric_unlock')
      if (ok) { unlocked.set(true); return }
      error = 'Biometric unlock failed. Enter your password.'
    } catch (e) {
      error = String(e)
    }
    biometricLoading = false
  }

  async function resetIdentity() {
    if (!confirm('Delete all local data and start over? This cannot be undone.')) return
    resetting = true
    try {
      await invoke('clear_identity')
      window.location.reload()
    } catch (e) {
      error = 'Reset failed: ' + e
      resetting = false
    }
  }

  async function handleSubmit() {
    if (!password) return
    loading = true; error = ''
    try {
      const restored = await invoke('unlock', { password })
      isFirstTime = !restored
      if (!hasBiometric) {
        offerBiometric = true
        // Don't unlock yet — wait for user response to biometric offer
      } else {
        unlocked.set(true)
      }
    } catch (e) {
      const msg = String(e)
      error = msg === 'incorrect_password' ? 'Incorrect password. Try again.' : msg
    } finally {
      loading = false
    }
  }

  async function enableBiometric() {
    try {
      await invoke('save_biometric_unlock')
      hasBiometric = true
    } catch (e) {
      error = 'Could not save biometric key: ' + e
    }
    offerBiometric = false
    unlocked.set(true)
  }

  function skipBiometric() {
    offerBiometric = false
    unlocked.set(true)
  }

  async function disableBiometric() {
    try {
      await invoke('delete_biometric_unlock')
      hasBiometric = false
    } catch (e) {
      error = String(e)
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

    {#if biometricLoading}
      <div class="biometric-status">
        <span class="spinner">⟳</span> Requesting biometric unlock…
      </div>
    {:else}
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

      {#if hasBiometric && !biometricLoading}
        <button class="biometric-btn" on:click={tryBiometric} disabled={loading}>
          🪪 Unlock with biometric
        </button>
      {/if}

      {#if offerBiometric}
        <div class="biometric-offer">
          <p>Enable biometric unlock (Windows Hello / Touch ID)?</p>
          <div class="bio-offer-btns">
            <button class="bio-yes" on:click={enableBiometric}>Enable</button>
            <button class="bio-no" on:click={skipBiometric}>Not now</button>
          </div>
        </div>
      {/if}

      {#if hasBiometric}
        <button class="reset-btn" on:click={disableBiometric}>Disable biometric unlock</button>
      {/if}
    {/if}

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

  .biometric-status {
    text-align: center;
    color: var(--text-muted);
    font-size: 14px;
    padding: 12px 0;
  }

  .biometric-btn {
    background: var(--bg-hover, #313244);
    color: var(--text);
    font-size: 14px;
    font-weight: 500;
    border: 1px solid var(--border);
    padding: 10px;
    border-radius: var(--radius);
  }
  .biometric-btn:not(:disabled):hover { border-color: var(--accent); }

  .biometric-offer {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px;
    font-size: 13px;
    color: var(--text-muted);
    text-align: center;
  }
  .biometric-offer p { margin: 0 0 10px; }
  .bio-offer-btns { display: flex; gap: 8px; }
  .bio-yes {
    flex: 1; padding: 7px; font-size: 13px; font-weight: 600;
    background: var(--accent); color: #1e1e2e; border: none;
    border-radius: 7px; cursor: pointer;
  }
  .bio-no {
    flex: 1; padding: 7px; font-size: 13px;
    background: none; color: var(--text-muted);
    border: 1px solid var(--border); border-radius: 7px; cursor: pointer;
  }
  .bio-yes:hover { filter: brightness(1.12); }
  .bio-no:hover { color: var(--text); }

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
