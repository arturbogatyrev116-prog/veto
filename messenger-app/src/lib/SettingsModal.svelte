<script>
  import { invoke } from '@tauri-apps/api/core'
  import { chatBg, CHAT_GRADIENTS } from '../stores.js'

  export let theme  = 'system'
  export let accent = 'blue'
  export let dndEnabled = false
  export let dndFrom    = '22:00'
  export let dndTo      = '08:00'
  export let screenProtected = false
  export let onClose = () => {}

  let soundEnabled = localStorage.getItem('sound_enabled') !== 'false'
  let fontSize     = localStorage.getItem('font_size') ?? 'default'
  let tab = 'appearance'

  const ACCENTS = {
    blue:   { label: 'Blue',   hex: '#3b82f6' },
    purple: { label: 'Purple', hex: '#8b5cf6' },
    green:  { label: 'Green',  hex: '#22c55e' },
    pink:   { label: 'Pink',   hex: '#ec4899' },
    orange: { label: 'Orange', hex: '#f97316' },
    teal:   { label: 'Teal',   hex: '#14b8a6' },
  }

  const FONT_SIZES = [
    { key: 'small',   label: 'Small',   px: '13px' },
    { key: 'default', label: 'Default', px: '14px' },
    { key: 'large',   label: 'Large',   px: '16px' },
  ]

  const SHORTCUTS = [
    { keys: ['Ctrl', 'K'],          desc: 'Search conversations' },
    { keys: ['Ctrl', 'F'],          desc: 'Search messages' },
    { keys: ['Ctrl', 'N'],          desc: 'New conversation' },
    { keys: ['Ctrl', 'Tab'],        desc: 'Next conversation' },
    { keys: ['Ctrl', '⇧', 'Tab'],  desc: 'Previous conversation' },
    { keys: ['Ctrl', 'W'],          desc: 'Close conversation' },
    { keys: ['/'],                  desc: 'Command palette' },
    { keys: ['Esc'],                desc: 'Close dialogs / cancel' },
    { keys: ['↑'],                  desc: 'Edit last sent message' },
  ]

  // Save DND to localStorage whenever it changes (App.svelte reads directly from localStorage)
  $: { localStorage.setItem('dnd_enabled', String(dndEnabled)); localStorage.setItem('dnd_from', dndFrom); localStorage.setItem('dnd_to', dndTo) }

  // Sound toggle
  $: localStorage.setItem('sound_enabled', String(soundEnabled))

  function setFontSize(key) {
    fontSize = key
    const px = FONT_SIZES.find(f => f.key === key)?.px ?? '14px'
    document.documentElement.style.setProperty('--app-font-size', px)
    localStorage.setItem('font_size', key)
  }

  // Apply saved font size on mount
  ;(() => {
    const px = FONT_SIZES.find(f => f.key === fontSize)?.px ?? '14px'
    document.documentElement.style.setProperty('--app-font-size', px)
  })()

  // Chat background image
  let bgImageSrc = localStorage.getItem('chat_bg_image') ?? ''

  function uploadBgImage() {
    const inp = document.createElement('input')
    inp.type = 'file'; inp.accept = 'image/*'
    inp.onchange = () => {
      const file = inp.files[0]; if (!file) return
      const reader = new FileReader()
      reader.onload = e => {
        bgImageSrc = e.target.result
        try { localStorage.setItem('chat_bg_image', bgImageSrc) } catch {}
        chatBg.update(v => ({ ...v, type: 'image' }))
      }
      reader.readAsDataURL(file)
    }
    inp.click()
  }

  function removeBgImage() {
    bgImageSrc = ''
    localStorage.removeItem('chat_bg_image')
    chatBg.update(v => ({ ...v, type: 'none' }))
  }

  function handleKeydown(e) {
    if (e.key === 'Escape') { e.stopPropagation(); onClose() }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<div class="sm-overlay" role="none" on:click|self={onClose}>
  <div class="sm-modal" role="dialog" aria-modal="true" aria-label="Settings">
    <div class="sm-header">
      <span class="sm-title">Settings</span>
      <button class="sm-close" on:click={onClose} aria-label="Close settings">✕</button>
    </div>

    <div class="sm-layout">
      <!-- Left nav -->
      <nav class="sm-nav">
        <button class="sm-tab" class:active={tab === 'appearance'}    on:click={() => tab = 'appearance'}>🎨 Appearance</button>
        <button class="sm-tab" class:active={tab === 'notifications'} on:click={() => tab = 'notifications'}>🔔 Notifications</button>
        <button class="sm-tab" class:active={tab === 'privacy'}       on:click={() => tab = 'privacy'}>🛡 Privacy</button>
        <button class="sm-tab" class:active={tab === 'shortcuts'}     on:click={() => tab = 'shortcuts'}>⌨ Shortcuts</button>
        <button class="sm-tab" class:active={tab === 'chat'}          on:click={() => tab = 'chat'}>💬 Chat</button>
      </nav>

      <!-- Content -->
      <div class="sm-content">

        {#if tab === 'appearance'}
          <div class="sm-section">
            <div class="sm-row">
              <div class="sm-row-label">
                <div class="sm-lbl">Theme</div>
              </div>
              <div class="sm-pills">
                <button class="sm-pill" class:selected={theme === 'dark'}   on:mousedown|preventDefault on:click={() => theme = 'dark'}>Dark</button>
                <button class="sm-pill" class:selected={theme === 'system'} on:mousedown|preventDefault on:click={() => theme = 'system'}>Auto</button>
                <button class="sm-pill" class:selected={theme === 'light'}  on:mousedown|preventDefault on:click={() => theme = 'light'}>Light</button>
              </div>
            </div>

            <div class="sm-row">
              <div class="sm-row-label">
                <div class="sm-lbl">Accent colour</div>
              </div>
              <div class="sm-swatches">
                {#each Object.entries(ACCENTS) as [key, p]}
                  <button
                    class="sm-swatch"
                    class:selected={accent === key}
                    style="background:{p.hex}"
                    aria-label={p.label}
                    title={p.label}
                    on:mousedown|preventDefault
                    on:click={() => accent = key}
                  ></button>
                {/each}
              </div>
            </div>

            <div class="sm-row">
              <div class="sm-row-label">
                <div class="sm-lbl">Text size</div>
              </div>
              <div class="sm-pills">
                {#each FONT_SIZES as f}
                  <button class="sm-pill" class:selected={fontSize === f.key} on:mousedown|preventDefault on:click={() => setFontSize(f.key)}>{f.label}</button>
                {/each}
              </div>
            </div>
          </div>

        {:else if tab === 'notifications'}
          <div class="sm-section">
            <div class="sm-row">
              <div class="sm-row-label">
                <div class="sm-lbl">Sound</div>
                <div class="sm-desc">Play a tone on new messages</div>
              </div>
              <label class="sm-toggle">
                <input type="checkbox" bind:checked={soundEnabled} />
                <span class="sm-toggle-track"></span>
              </label>
            </div>

            <div class="sm-row">
              <div class="sm-row-label">
                <div class="sm-lbl">Do Not Disturb</div>
                <div class="sm-desc">Silence all notifications</div>
              </div>
              <label class="sm-toggle">
                <input type="checkbox" bind:checked={dndEnabled} />
                <span class="sm-toggle-track"></span>
              </label>
            </div>

            {#if dndEnabled}
              <div class="sm-row sm-row-sub">
                <div class="sm-row-label">
                  <div class="sm-lbl">Quiet hours</div>
                  <div class="sm-desc">Silence during this window each day</div>
                </div>
                <div class="sm-time-pair">
                  <input type="time" bind:value={dndFrom} />
                  <span class="sm-time-sep">→</span>
                  <input type="time" bind:value={dndTo} />
                </div>
              </div>
            {/if}
          </div>

        {:else if tab === 'privacy'}
          <div class="sm-section">
            <div class="sm-row">
              <div class="sm-row-label">
                <div class="sm-lbl">Screen capture protection</div>
                <div class="sm-desc">Prevent screenshots and screen recording of the app window</div>
              </div>
              <label class="sm-toggle">
                <input type="checkbox" bind:checked={screenProtected} />
                <span class="sm-toggle-track"></span>
              </label>
            </div>
          </div>

        {:else if tab === 'shortcuts'}
          <div class="sm-section">
            <table class="sm-keys-table">
              <tbody>
                {#each SHORTCUTS as s}
                  <tr>
                    <td class="sm-keys-cell">
                      {#each s.keys as k}<kbd>{k}</kbd>{/each}
                    </td>
                    <td class="sm-keys-desc">{s.desc}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else if tab === 'chat'}
          <div class="sm-section">
            <div class="sm-row">
              <div class="sm-row-label"><div class="sm-lbl">Background</div></div>
              <div class="sm-pills">
                <button class="sm-pill" class:selected={$chatBg.type === 'none'}
                  on:mousedown|preventDefault on:click={() => chatBg.update(v => ({...v, type: 'none'}))}>None</button>
                <button class="sm-pill" class:selected={$chatBg.type === 'gradient'}
                  on:mousedown|preventDefault on:click={() => chatBg.update(v => ({...v, type: 'gradient'}))}>Gradient</button>
                <button class="sm-pill" class:selected={$chatBg.type === 'image'}
                  on:mousedown|preventDefault on:click={uploadBgImage}>Image…</button>
              </div>
            </div>

            {#if $chatBg.type === 'gradient'}
              <div class="sm-row">
                <div class="sm-row-label"><div class="sm-lbl">Style</div></div>
                <div class="bg-swatches">
                  {#each Object.entries(CHAT_GRADIENTS) as [key, g]}
                    <button
                      class="bg-swatch"
                      class:selected={$chatBg.gradient === key}
                      style="background:{g.css}"
                      title={g.label}
                      aria-label={g.label}
                      on:mousedown|preventDefault
                      on:click={() => chatBg.update(v => ({...v, gradient: key}))}
                    ></button>
                  {/each}
                </div>
              </div>
            {/if}

            {#if $chatBg.type === 'image'}
              <div class="sm-row">
                <div class="sm-row-label"><div class="sm-lbl">Image</div></div>
                <div class="bg-img-row">
                  {#if bgImageSrc}
                    <div class="bg-thumb" style="background:url('{bgImageSrc}') center/cover"></div>
                  {/if}
                  <button class="sm-pill" on:click={uploadBgImage}>Change…</button>
                  {#if bgImageSrc}
                    <button class="sm-pill" on:click={removeBgImage}>Remove</button>
                  {/if}
                </div>
              </div>
            {/if}

            {#if $chatBg.type !== 'none'}
              <div class="sm-row">
                <div class="sm-row-label">
                  <div class="sm-lbl">Blur</div>
                  <div class="sm-desc">{$chatBg.blur === 0 ? 'Off' : `${$chatBg.blur} px`}</div>
                </div>
                <input type="range" min="0" max="10" step="1" class="sm-range"
                  value={$chatBg.blur}
                  on:input={e => chatBg.update(v => ({...v, blur: +e.target.value}))} />
              </div>
              <div class="sm-row">
                <div class="sm-row-label">
                  <div class="sm-lbl">Overlay dim</div>
                  <div class="sm-desc">{$chatBg.dim === 0 ? 'Off' : `${$chatBg.dim}%`}</div>
                </div>
                <input type="range" min="0" max="80" step="5" class="sm-range"
                  value={$chatBg.dim}
                  on:input={e => chatBg.update(v => ({...v, dim: +e.target.value}))} />
              </div>
            {/if}
          </div>
        {/if}

      </div>
    </div>
  </div>
</div>

<style>
  .sm-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    backdrop-filter: blur(2px);
    z-index: 300;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .sm-modal {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
    width: 580px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 60px);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    animation: sm-in 0.14s ease;
  }
  @keyframes sm-in {
    from { opacity: 0; transform: scale(0.96) translateY(6px); }
    to   { opacity: 1; transform: none; }
  }

  .sm-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .sm-title { font-weight: 700; font-size: 15px; color: var(--text); }
  .sm-close {
    background: none; color: var(--text-muted);
    font-size: 15px; padding: 4px 7px; border-radius: 6px; line-height: 1;
  }
  .sm-close:hover { background: var(--bg-hover); color: var(--text); }

  /* Layout: left nav + right content */
  .sm-layout { display: flex; flex: 1; overflow: hidden; }

  .sm-nav {
    width: 152px;
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    padding: 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: var(--bg);
  }
  .sm-tab {
    background: none; color: var(--text-muted);
    font-size: 13px; padding: 8px 10px;
    border-radius: 7px; text-align: left;
    transition: background 0.12s, color 0.12s;
  }
  .sm-tab:hover { background: var(--bg-hover); color: var(--text); }
  .sm-tab.active { background: var(--bg-active); color: var(--accent); font-weight: 600; }

  .sm-content { flex: 1; overflow-y: auto; padding: 16px 20px; }

  /* Section rows */
  .sm-section { display: flex; flex-direction: column; }
  .sm-row {
    display: flex; align-items: center; justify-content: space-between;
    gap: 16px; padding: 13px 0;
    border-bottom: 1px solid var(--border-sub);
  }
  .sm-row:last-child { border-bottom: none; }
  .sm-row-sub { padding-left: 12px; }
  .sm-row-label { display: flex; flex-direction: column; gap: 3px; flex: 1; min-width: 0; }
  .sm-lbl { font-size: 13px; color: var(--text); font-weight: 500; }
  .sm-desc { font-size: 11px; color: var(--text-dim); line-height: 1.4; }

  /* Pills */
  .sm-pills { display: flex; gap: 4px; flex-shrink: 0; }
  .sm-pill {
    background: var(--bg-hover); color: var(--text-muted);
    font-size: 11px; padding: 4px 10px;
    border-radius: 6px; border: 1px solid var(--border);
    cursor: pointer; transition: background 0.12s, color 0.12s;
  }
  .sm-pill:hover { color: var(--text); }
  .sm-pill.selected { background: var(--accent); color: #fff; border-color: var(--accent); }

  /* Accent swatches */
  .sm-swatches { display: flex; gap: 6px; flex-shrink: 0; }
  .sm-swatch {
    width: 24px; height: 24px; border-radius: 50%;
    border: 2px solid transparent; padding: 0; cursor: pointer; flex-shrink: 0;
    transition: transform 0.12s, border-color 0.12s;
  }
  .sm-swatch:hover { transform: scale(1.15); }
  .sm-swatch.selected { border-color: var(--text); transform: scale(1.15); }

  /* Toggle switch */
  .sm-toggle { position: relative; display: inline-flex; cursor: pointer; flex-shrink: 0; }
  .sm-toggle input { position: absolute; opacity: 0; width: 0; height: 0; }
  .sm-toggle-track {
    width: 40px; height: 22px;
    background: var(--bg-hover); border: 1px solid var(--border);
    border-radius: 11px; position: relative;
    transition: background 0.18s, border-color 0.18s;
  }
  .sm-toggle-track::after {
    content: '';
    position: absolute; top: 2px; left: 2px;
    width: 16px; height: 16px; border-radius: 50%;
    background: var(--text-muted);
    transition: transform 0.18s, background 0.18s;
  }
  .sm-toggle input:checked + .sm-toggle-track { background: var(--accent); border-color: var(--accent); }
  .sm-toggle input:checked + .sm-toggle-track::after { transform: translateX(18px); background: #fff; }

  /* Time inputs */
  .sm-time-pair { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
  .sm-time-pair input[type="time"] { font-size: 12px; padding: 4px 6px; width: 88px; }
  .sm-time-sep { color: var(--text-dim); font-size: 13px; }

  /* Chat background */
  .bg-swatches { display: flex; gap: 6px; flex-wrap: wrap; flex-shrink: 0; }
  .bg-swatch {
    width: 36px; height: 36px; border-radius: 6px;
    border: 2px solid transparent; padding: 0; cursor: pointer; flex-shrink: 0;
    transition: transform 0.12s, border-color 0.12s;
  }
  .bg-swatch:hover { transform: scale(1.1); }
  .bg-swatch.selected { border-color: var(--text); transform: scale(1.1); }
  .bg-img-row { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
  .bg-thumb { width: 52px; height: 38px; border-radius: 6px; flex-shrink: 0; border: 1px solid var(--border); }
  .sm-range {
    width: 130px; flex-shrink: 0; cursor: pointer;
    height: 4px; border-radius: 2px; border: none; padding: 0;
    accent-color: var(--accent);
    background: var(--border);
  }

  /* Shortcuts table */
  .sm-keys-table { border-collapse: collapse; width: 100%; }
  .sm-keys-table tr { border-bottom: 1px solid var(--border-sub); }
  .sm-keys-table tr:last-child { border-bottom: none; }
  .sm-keys-cell { padding: 10px 14px 10px 0; white-space: nowrap; }
  .sm-keys-cell kbd {
    display: inline-block;
    background: var(--bg-hover); border: 1px solid var(--border);
    border-radius: 4px; padding: 1px 6px;
    font-size: 11px; font-family: inherit;
    color: var(--text-muted); margin-right: 2px;
  }
  .sm-keys-desc { font-size: 12px; color: var(--text-muted); padding: 10px 0; }
</style>
