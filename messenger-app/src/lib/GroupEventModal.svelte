<script>
  import { invoke } from '@tauri-apps/api/core'
  import { createEventDispatcher } from 'svelte'

  export let groupId

  const dispatch = createEventDispatcher()

  let title = ''
  let dateStr = ''
  let timeStr = ''
  let location = ''
  let description = ''
  let reminderMin = 15
  let sending = false
  let error = ''

  const REMINDERS = [
    { label: 'None', value: 0 },
    { label: '5 min', value: 5 },
    { label: '15 min', value: 15 },
    { label: '30 min', value: 30 },
    { label: '1 hour', value: 60 },
    { label: '1 day', value: 1440 },
  ]

  async function create() {
    const t = title.trim()
    if (!t || !dateStr || !timeStr) { error = 'Title and date/time are required'; return }
    sending = true; error = ''
    try {
      const dateMs = new Date(`${dateStr}T${timeStr}`).getTime()
      await invoke('send_group_message', {
        groupId,
        text: '',
        replyTo: null,
        mentions: [],
        payloadOverride: JSON.stringify({
          event: {
            title: t,
            date_ms: dateMs,
            location: location.trim() || null,
            desc: description.trim() || null,
            reminder_ms: reminderMin * 60_000,
          }
        })
      })
      dispatch('sent')
    } catch (e) {
      error = String(e)
    } finally {
      sending = false
    }
  }
</script>

<div class="modal-overlay" role="dialog" aria-modal="true" on:click|self={() => dispatch('close')}>
  <div class="modal-box">
    <div class="modal-title">📅 Create Event</div>

    <label class="field-label">Title *
      <input class="modal-input" type="text" bind:value={title} placeholder="Event name" maxlength="100" autofocus />
    </label>

    <div class="date-row">
      <label class="field-label half">Date *
        <input class="modal-input" type="date" bind:value={dateStr} />
      </label>
      <label class="field-label half">Time *
        <input class="modal-input" type="time" bind:value={timeStr} />
      </label>
    </div>

    <label class="field-label">Location
      <input class="modal-input" type="text" bind:value={location} placeholder="Optional" maxlength="200" />
    </label>

    <label class="field-label">Description
      <textarea class="modal-input desc-area" bind:value={description} placeholder="Optional" maxlength="500" rows="3"></textarea>
    </label>

    <label class="field-label">Reminder
      <select class="modal-input" bind:value={reminderMin}>
        {#each REMINDERS as r}
          <option value={r.value}>{r.label}</option>
        {/each}
      </select>
    </label>

    {#if error}<p class="err">{error}</p>{/if}

    <div class="modal-btns">
      <button
        class="modal-ok"
        on:click={create}
        disabled={sending || !title.trim() || !dateStr || !timeStr}
      >{sending ? 'Sending…' : 'Create Event'}</button>
      <button class="modal-cancel" on:click={() => dispatch('close')}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .modal-overlay {
    position: fixed; inset: 0;
    background: rgba(0,0,0,0.55);
    display: flex; align-items: center; justify-content: center;
    z-index: 200;
  }
  .modal-box {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 20px 24px;
    width: 360px;
    max-width: calc(100vw - 32px);
    display: flex; flex-direction: column; gap: 10px;
    box-shadow: 0 8px 32px rgba(0,0,0,0.3);
  }
  .modal-title { font-weight: 700; font-size: 16px; color: var(--text); }
  .field-label {
    display: flex; flex-direction: column; gap: 4px;
    font-size: 12px; color: var(--text-muted);
  }
  .field-label.half { flex: 1; }
  .date-row { display: flex; gap: 8px; }
  .modal-input {
    width: 100%; box-sizing: border-box;
    background: var(--bg); border: 1px solid var(--border);
    color: var(--text); border-radius: var(--radius-sm, 6px);
    padding: 7px 10px; font-size: 13px;
  }
  .desc-area { resize: vertical; }
  .err { color: var(--danger); font-size: 12px; margin: 0; }
  .modal-btns { display: flex; gap: 8px; margin-top: 4px; }
  .modal-ok {
    flex: 1; padding: 8px; border-radius: var(--radius-sm, 6px);
    background: var(--accent); color: #fff; font-weight: 600; font-size: 13px;
  }
  .modal-ok:disabled { opacity: 0.5; cursor: not-allowed; }
  .modal-cancel {
    flex: 1; padding: 8px; border-radius: var(--radius-sm, 6px);
    background: var(--bg-hover); color: var(--text); font-size: 13px;
  }
</style>
