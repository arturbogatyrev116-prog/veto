<script>
  export let onSelect = null
  export let onClose = null

  const PACKS = {
    smiles: { label: '😊', stickers: ['😊','😂','🤣','😍','🥰','😘','😎','🤩','😏','😒','😭','😤','😡','🥺','😱','🤔','😴','🤗','😋','🙄'] },
    gestures: { label: '👍', stickers: ['👍','👎','👏','🙌','🤝','✌️','🤞','👋','🤜','🤛','💪','🙏','👌','🤙','☝️','✊','🫶','🫂','🤲','💅'] },
    hearts: { label: '❤️', stickers: ['❤️','🧡','💛','💚','💙','💜','🖤','🤍','💔','💕','💞','💓','💗','💖','💝','💘','💟','❣️','🫀','💌'] },
    nature: { label: '🌸', stickers: ['🌸','🌺','🌻','🌼','🌷','🌹','🌿','🍀','🍁','🌊','⭐','🌙','☀️','❄️','🌈','🔥','💧','⚡','🌙','🍄'] },
    food: { label: '🍕', stickers: ['🍕','🍔','🍟','🌮','🍣','🍜','🍦','🎂','🍩','🍪','☕','🧃','🍺','🎉','🎊','🎈','🎁','🏆','🎮','⚽'] },
  }

  let activeTab = 'smiles'

  const recent = JSON.parse(localStorage.getItem('sticker_recent') ?? '[]')

  function select(emoji) {
    const already = recent.indexOf(emoji)
    if (already !== -1) recent.splice(already, 1)
    recent.unshift(emoji)
    if (recent.length > 16) recent.length = 16
    localStorage.setItem('sticker_recent', JSON.stringify(recent))
    onSelect?.({ pack: activeTab, id: emoji })
  }
</script>

<div class="picker">
  <div class="tabs">
    {#if recent.length > 0}
      <button class:active={activeTab === 'recent'} on:click={() => activeTab = 'recent'} title="Recent">🕐</button>
    {/if}
    {#each Object.entries(PACKS) as [key, pack]}
      <button class:active={activeTab === key} on:click={() => activeTab = key} title={key}>{pack.label}</button>
    {/each}
  </div>

  <div class="grid">
    {#if activeTab === 'recent'}
      {#each recent as s}
        <button class="sticker-btn" on:click={() => select(s)}>{s}</button>
      {/each}
    {:else}
      {#each PACKS[activeTab].stickers as s}
        <button class="sticker-btn" on:click={() => select(s)}>{s}</button>
      {/each}
    {/if}
  </div>
</div>

<style>
  .picker {
    width: 280px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 4px 20px rgba(0,0,0,0.25);
    overflow: hidden;
  }
  .tabs {
    display: flex;
    border-bottom: 1px solid var(--border);
    padding: 4px 6px;
    gap: 2px;
  }
  .tabs button {
    background: none;
    padding: 4px 6px;
    font-size: 18px;
    border-radius: 6px;
    line-height: 1;
    opacity: 0.6;
    transition: opacity 0.1s, background 0.1s;
  }
  .tabs button:hover { background: var(--bg-hover); opacity: 1; }
  .tabs button.active { background: color-mix(in srgb, var(--accent) 15%, transparent); opacity: 1; }

  .grid {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 2px;
    padding: 6px;
    max-height: 200px;
    overflow-y: auto;
  }
  .sticker-btn {
    background: none;
    font-size: 28px;
    padding: 4px;
    border-radius: 8px;
    line-height: 1;
    transition: background 0.1s, transform 0.1s;
    cursor: pointer;
  }
  .sticker-btn:hover { background: var(--bg-hover); transform: scale(1.15); }
</style>
