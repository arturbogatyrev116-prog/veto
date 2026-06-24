<script>
  export let onSubmit;
  export let onCancel;

  let username = '';
  let error = '';
  let loading = false;
  // Token is shown here after successful creation (only time it's visible).
  let createdToken = '';

  async function handleSubmit() {
    error = '';
    username = username.trim();
    if (!username) { error = 'Username is required'; return; }
    if (username.length > 64) { error = 'Username must be ≤ 64 characters'; return; }

    loading = true;
    try {
      const result = await onSubmit(username);
      createdToken = result?.token ?? '';
      if (!createdToken) onCancel();
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  function copyToken() {
    navigator.clipboard.writeText(createdToken).catch(() => {});
  }

  function done() {
    createdToken = '';
    username = '';
    onCancel();
  }
</script>

<div class="form-card">
  {#if createdToken}
    <h2>User created</h2>
    <p class="hint">
      Copy this bearer token now — it will <strong>not</strong> be shown again.
    </p>
    <div class="token-box">
      <code>{createdToken}</code>
      <button class="btn copy" on:click={copyToken}>Copy</button>
    </div>
    <div class="row">
      <button class="btn primary" on:click={done}>Done</button>
    </div>
  {:else}
    <h2>Create user</h2>

    <label>
      Username
      <input
        type="text"
        bind:value={username}
        placeholder="e.g. alice"
        on:keydown={(e) => e.key === 'Enter' && handleSubmit()}
        disabled={loading}
        autofocus
      />
    </label>

    {#if error}
      <div class="error">{error}</div>
    {/if}

    <div class="row">
      <button class="btn primary" on:click={handleSubmit} disabled={loading}>
        {loading ? 'Creating…' : 'Create'}
      </button>
      <button class="btn secondary" on:click={onCancel} disabled={loading}>Cancel</button>
    </div>
  {/if}
</div>

<style>
  .form-card {
    background: white;
    border-radius: 8px;
    box-shadow: 0 1px 4px rgba(0,0,0,.1);
    padding: 24px;
    margin-bottom: 20px;
    max-width: 480px;
  }

  h2 { margin: 0 0 16px; font-size: 18px; }

  label {
    display: block;
    font-size: 13px;
    font-weight: 600;
    color: #374151;
    margin-bottom: 16px;
  }

  input {
    display: block;
    width: 100%;
    margin-top: 6px;
    padding: 9px 12px;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    font-size: 14px;
    box-sizing: border-box;
  }
  input:focus {
    outline: none;
    border-color: #6366f1;
    box-shadow: 0 0 0 3px rgba(99,102,241,.15);
  }

  .hint { font-size: 13px; color: #6b7280; margin-bottom: 12px; }

  .token-box {
    display: flex;
    align-items: center;
    gap: 8px;
    background: #f3f4f6;
    border-radius: 6px;
    padding: 10px 14px;
    margin-bottom: 20px;
    overflow-x: auto;
  }
  code {
    font-family: monospace;
    font-size: 13px;
    flex: 1;
    word-break: break-all;
  }

  .error {
    color: #dc2626;
    font-size: 13px;
    margin-bottom: 12px;
  }

  .row { display: flex; gap: 8px; }

  .btn {
    padding: 8px 18px;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: opacity .15s;
  }
  .btn:disabled { opacity: .5; cursor: default; }
  .btn:hover:not(:disabled) { opacity: .88; }

  .btn.primary   { background: #6366f1; color: #fff; }
  .btn.secondary { background: #e5e7eb; color: #374151; }
  .btn.copy      { background: #e5e7eb; color: #374151; flex-shrink: 0; }
</style>
