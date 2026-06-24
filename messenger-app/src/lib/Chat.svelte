<script>
  import Sidebar from './Sidebar.svelte'
  import MessagePane from './MessagePane.svelte'
  import { activeConv } from '../stores.js'

  let sidebarRef

  export function focusSearch() { sidebarRef?.focusSearch?.() }
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
    height: 100vh;
  }
  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-dim);
  }
</style>
