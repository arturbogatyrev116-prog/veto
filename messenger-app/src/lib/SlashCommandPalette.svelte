<script>
  export let query = ''       // text after '/'
  export let isGroup = false
  export let onSelect = null  // (cmd) => void

  const ALL_COMMANDS = [
    { cmd: 'clear',    icon: '🗑',  label: 'Clear history',        desc: 'Delete all local messages',        group: false },
    { cmd: 'mute',     icon: '🔕',  label: 'Mute',                 desc: 'Mute: 1h | 8h | 1w | off',        group: false, args: ['1h', '8h', '1w', 'off'] },
    { cmd: 'ttl',      icon: '⏱',  label: 'Disappearing messages', desc: 'Set TTL: off | 1h | 1d | 1w',     group: false, args: ['off', '1h', '1d', '1w'] },
    { cmd: 'export',   icon: '⬇',  label: 'Export chat',           desc: 'Export: json | html | md',         group: false, args: ['json', 'html', 'md'] },
    { cmd: 'search',   icon: '🔍',  label: 'Search',                desc: 'Search in conversation',           group: false },
    { cmd: 'poll',     icon: '📊',  label: 'Create poll',           desc: 'Open poll wizard',                 group: true  },
    { cmd: 'schedule', icon: '🕐',  label: 'Schedule message',      desc: 'Send at a specific time',          group: false },
  ]

  $: filtered = (() => {
    const q = query.toLowerCase()
    return ALL_COMMANDS.filter(c => {
      if (c.group && !isGroup) return false
      return c.cmd.startsWith(q) || c.label.toLowerCase().includes(q)
    })
  })()

  let focusIdx = 0
  $: focusIdx = 0  // reset on filter change

  export function moveDown() {
    focusIdx = Math.min(focusIdx + 1, filtered.length - 1)
  }
  export function moveUp() {
    focusIdx = Math.max(focusIdx - 1, 0)
  }
  export function confirm() {
    if (filtered[focusIdx]) onSelect?.(filtered[focusIdx])
  }
</script>

{#if filtered.length > 0}
  <div class="slash-palette" role="listbox" aria-label="Commands">
    {#each filtered as c, i (c.cmd)}
      <button
        type="button"
        class="slash-item"
        class:focused={i === focusIdx}
        role="option"
        aria-selected={i === focusIdx}
        on:mousedown|preventDefault={() => onSelect?.(c)}
        on:mousemove={() => { focusIdx = i }}
      >
        <span class="slash-icon">{c.icon}</span>
        <span class="slash-label">/{c.cmd}</span>
        <span class="slash-desc">{c.desc}</span>
        {#if c.args}
          <span class="slash-args">{c.args.join(' | ')}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .slash-palette {
    background: var(--bg-panel);
    border-top: 1px solid var(--border);
    overflow-y: auto;
    max-height: 220px;
    flex-shrink: 0;
  }
  .slash-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    padding: 8px 14px;
    background: none;
    border-radius: 0;
    cursor: pointer;
    transition: background 0.1s;
    min-height: 38px;
  }
  .slash-item:hover, .slash-item.focused {
    background: var(--bg-hover);
    outline: none;
  }
  .slash-icon { font-size: 16px; width: 22px; flex-shrink: 0; }
  .slash-label { font-size: 13px; font-weight: 700; color: var(--accent); min-width: 90px; }
  .slash-desc { font-size: 12px; color: var(--text-muted); flex: 1; }
  .slash-args {
    font-size: 11px;
    color: var(--text-dim);
    font-family: monospace;
    flex-shrink: 0;
  }
</style>
