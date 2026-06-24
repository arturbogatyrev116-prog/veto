<script>
  export let name = ''   // display name or user_id
  export let uid = ''    // used for color derivation
  export let size = 32   // px

  function initials(s) {
    const parts = s.trim().split(/\s+/)
    if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase()
    return s.slice(0, 2).toUpperCase()
  }

  // Deterministic hue from uid string
  function hue(s) {
    let h = 0
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) & 0xffffffff
    return Math.abs(h) % 360
  }

  $: h = hue(uid || name)
  $: bg = `hsl(${h}, 55%, 38%)`
  $: label = initials(name || uid)
  $: font = Math.round(size * 0.38)
</script>

<div
  class="avatar"
  style="width:{size}px;height:{size}px;background:{bg};font-size:{font}px;"
  title={name}
>
  {label}
</div>

<style>
  .avatar {
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #fff;
    font-weight: 600;
    flex-shrink: 0;
    user-select: none;
  }
</style>
