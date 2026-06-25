<script>
  import Sidebar from './Sidebar.svelte'
  import MessagePane from './MessagePane.svelte'
  import { activeConv, conversations, groups } from '../stores.js'

  let sidebarRef

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

<div class="chat-layout">
  <Sidebar bind:this={sidebarRef} />
  {#if $activeConv}
    <MessagePane peerId={$activeConv} />
  {:else}
    <div class="empty">Select a conversation or add one with +</div>
  {/if}
</div>

<style>
  .chat-layout {
    display: flex;
    height: 100%;
    min-height: 0;
  }
  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-dim);
  }
</style>
