<script>
  import Sidebar from './Sidebar.svelte'
  import MessagePane from './MessagePane.svelte'
  import DetailPanel from './DetailPanel.svelte'
  import { activeConv, conversations, groups } from '../stores.js'

  let sidebarRef
  let innerWidth = 1200
  let showDetail = false

  // Persist mobile sidebar open state in localStorage
  let sidebarOpen = (() => {
    try { return JSON.parse(localStorage.getItem('sidebarOpen') ?? 'true') } catch { return true }
  })()

  $: collapsed = innerWidth >= 600 && innerWidth < 1000
  $: mobileMode = innerWidth < 600

  // Auto-close sidebar overlay when user picks a conversation on mobile
  $: if ($activeConv && mobileMode) sidebarOpen = false

  function toggleSidebar() {
    sidebarOpen = !sidebarOpen
    try { localStorage.setItem('sidebarOpen', JSON.stringify(sidebarOpen)) } catch {}
  }

  export function focusSearch() { sidebarRef?.focusSearch?.() }
  export function focusNewChat() { sidebarRef?.focusNewChat?.() }

  export function prevConv() {
    const ids = getConvIds()
    if (!ids.length) return
    const idx = ids.indexOf($activeConv)
    activeConv.set(ids[Math.max(0, idx - 1)])
  }

  export function nextConv() {
    const ids = getConvIds()
    if (!ids.length) return
    const idx = ids.indexOf($activeConv)
    activeConv.set(ids[Math.min(ids.length - 1, idx + 1)])
  }

  function getConvIds() {
    return [
      ...Object.keys($conversations),
      ...Object.values($groups).map(g => g.group_id),
    ]
  }
</script>

<svelte:window bind:innerWidth />

<div class="chat-layout" class:mobile={mobileMode} class:sidebar-open={mobileMode && sidebarOpen}>
  <!-- Mobile overlay backdrop -->
  {#if mobileMode && sidebarOpen}
    <div class="sidebar-overlay" on:click={toggleSidebar} role="none" />
  {/if}

  <div class="sidebar-wrap">
    <Sidebar bind:this={sidebarRef} collapsed={collapsed && !mobileMode} />
  </div>

  <div class="chat-main">
    {#if $activeConv}
      <MessagePane
        peerId={$activeConv}
        onToggleSidebar={mobileMode ? toggleSidebar : null}
        onToggleDetail={() => showDetail = !showDetail}
        detailOpen={showDetail}
      />
    {:else}
      <div class="empty">
        {#if mobileMode}
          <button class="open-sidebar-hint" on:click={toggleSidebar}>☰ Open conversations</button>
        {:else}
          Select a conversation or add one with +
        {/if}
      </div>
    {/if}
  </div>

  {#if showDetail && $activeConv}
    <DetailPanel peerId={$activeConv} onClose={() => showDetail = false} />
  {/if}
</div>

<style>
  .chat-layout {
    display: flex;
    height: 100%;
    min-height: 0;
    position: relative;
    overflow: hidden;
  }

  /* ── Sidebar wrap ── */
  .sidebar-wrap {
    flex-shrink: 0;
    height: 100%;
    transition: transform 0.22s cubic-bezier(0.4, 0, 0.2, 1);
  }

  /* Mobile: sidebar is off-screen by default */
  .chat-layout.mobile .sidebar-wrap {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    z-index: 100;
    transform: translateX(-100%);
  }
  .chat-layout.mobile.sidebar-open .sidebar-wrap {
    transform: translateX(0);
  }

  /* Mobile overlay backdrop */
  .sidebar-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 99;
  }

  /* ── Main chat area ── */
  .chat-main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    position: relative;
  }

  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-dim);
  }

  .open-sidebar-hint {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    color: var(--text-muted);
    padding: 10px 20px;
    border-radius: var(--radius);
    font-size: 14px;
    cursor: pointer;
  }
  .open-sidebar-hint:hover { color: var(--text); }
</style>
