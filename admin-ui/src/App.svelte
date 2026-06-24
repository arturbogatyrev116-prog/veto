<script>
  import { onMount } from 'svelte';
  import { api, setAdminToken, getAdminToken } from './lib/api.js';
  import UserList from './lib/UserList.svelte';
  import UserForm from './lib/UserForm.svelte';

  let authed = false;
  let tokenInput = '';
  let loginError = '';
  let users = [];
  let showCreate = false;
  let globalError = '';
  let refreshing = false;

  onMount(async () => {
    const saved = getAdminToken();
    if (saved) {
      tokenInput = saved;
      await tryLogin();
    }
  });

  async function tryLogin() {
    loginError = '';
    setAdminToken(tokenInput);
    try {
      users = await api.listUsers();
      authed = true;
    } catch {
      loginError = 'Invalid token or server unreachable.';
      setAdminToken('');
    }
  }

  function logout() {
    setAdminToken('');
    authed = false;
    tokenInput = '';
    users = [];
  }

  async function refresh() {
    refreshing = true;
    globalError = '';
    try { users = await api.listUsers(); }
    catch (e) { globalError = e.message; }
    finally { refreshing = false; }
  }

  async function handleCreate(username) {
    const result = await api.createUser(username);
    await refresh();
    return result; // UserForm will display the token
  }

  async function handleBlock(userId, reason) {
    globalError = '';
    try { await api.blockUser(userId, reason); await refresh(); }
    catch (e) { globalError = e.message; }
  }

  async function handleUnblock(userId) {
    globalError = '';
    try { await api.unblockUser(userId); await refresh(); }
    catch (e) { globalError = e.message; }
  }

  async function handleDelete(userId) {
    const u = users.find(u => u.user_id === userId);
    if (!u) return;
    if (!confirm(`Delete user "${u.username}"? This cannot be undone.`)) return;
    globalError = '';
    try { await api.deleteUser(userId); await refresh(); }
    catch (e) { globalError = e.message; }
  }
</script>

{#if !authed}
  <div class="login-wrap">
    <div class="login-card">
      <div class="logo">Veto Admin</div>
      <input
        type="password"
        bind:value={tokenInput}
        placeholder="Admin token"
        autocomplete="current-password"
        on:keydown={(e) => e.key === 'Enter' && tryLogin()}
      />
      <button on:click={tryLogin}>Sign in</button>
      {#if loginError}
        <div class="login-error">{loginError}</div>
      {/if}
    </div>
  </div>
{:else}
  <div class="layout">
    <header>
      <span class="brand">Veto Admin</span>
      <div class="header-actions">
        <button class="hdr-btn create" on:click={() => showCreate = !showCreate}>
          {showCreate ? '✕ Cancel' : '+ New user'}
        </button>
        <button class="hdr-btn" on:click={refresh} disabled={refreshing}>
          {refreshing ? '…' : '↻ Refresh'}
        </button>
        <button class="hdr-btn logout" on:click={logout}>Sign out</button>
      </div>
    </header>

    <main>
      {#if globalError}
        <div class="alert-error">{globalError}</div>
      {/if}

      {#if showCreate}
        <UserForm
          onSubmit={handleCreate}
          onCancel={() => showCreate = false}
        />
      {/if}

      <UserList
        {users}
        onBlock={handleBlock}
        onUnblock={handleUnblock}
        onDelete={handleDelete}
      />
    </main>
  </div>
{/if}

<style>
  :global(*, *::before, *::after) { box-sizing: border-box; }
  :global(body) {
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: #f3f4f6;
    color: #111827;
  }

  /* ── Login ── */
  .login-wrap {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .login-card {
    background: white;
    border-radius: 12px;
    box-shadow: 0 4px 24px rgba(0,0,0,.08);
    padding: 40px 36px;
    width: 100%;
    max-width: 360px;
    text-align: center;
  }

  .logo {
    font-size: 22px;
    font-weight: 700;
    margin-bottom: 28px;
    color: #6366f1;
    letter-spacing: -.5px;
  }

  .login-card input {
    width: 100%;
    padding: 11px 14px;
    border: 1px solid #d1d5db;
    border-radius: 8px;
    font-size: 15px;
    margin-bottom: 12px;
  }
  .login-card input:focus {
    outline: none;
    border-color: #6366f1;
    box-shadow: 0 0 0 3px rgba(99,102,241,.15);
  }

  .login-card button {
    width: 100%;
    padding: 11px;
    background: #6366f1;
    color: white;
    border: none;
    border-radius: 8px;
    font-size: 15px;
    font-weight: 600;
    cursor: pointer;
  }
  .login-card button:hover { background: #4f46e5; }

  .login-error {
    color: #dc2626;
    font-size: 13px;
    margin-top: 12px;
  }

  /* ── Layout ── */
  .layout { min-height: 100vh; display: flex; flex-direction: column; }

  header {
    background: white;
    border-bottom: 1px solid #e5e7eb;
    padding: 0 24px;
    height: 56px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    position: sticky;
    top: 0;
    z-index: 10;
  }

  .brand {
    font-weight: 700;
    font-size: 16px;
    color: #6366f1;
    letter-spacing: -.4px;
  }

  .header-actions { display: flex; gap: 8px; }

  .hdr-btn {
    padding: 6px 14px;
    border: 1px solid #e5e7eb;
    border-radius: 6px;
    background: white;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    color: #374151;
  }
  .hdr-btn:hover { background: #f9fafb; }
  .hdr-btn:disabled { opacity: .5; cursor: default; }
  .hdr-btn.create  { background: #6366f1; color: white; border-color: #6366f1; }
  .hdr-btn.create:hover { background: #4f46e5; }
  .hdr-btn.logout  { color: #dc2626; }

  main { flex: 1; padding: 24px; max-width: 1280px; margin: 0 auto; width: 100%; }

  .alert-error {
    background: #fee2e2;
    color: #991b1b;
    border-radius: 8px;
    padding: 12px 16px;
    font-size: 14px;
    margin-bottom: 16px;
  }
</style>
