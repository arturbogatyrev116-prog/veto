<script>
  export let preview  // { url, title, description, image_url, domain }
  $: isValidImage = preview.image_url &&
    (preview.image_url.startsWith('http://') || preview.image_url.startsWith('https://'))

  function isValidUrl(url) {
    try {
      const parsed = new URL(url)
      return parsed.protocol === 'http:' || parsed.protocol === 'https:'
    } catch {
      return false
    }
  }
  $: validUrl = isValidUrl(preview.url)
</script>

{#if validUrl}
<a class="link-preview" href={preview.url} target="_blank" rel="noopener noreferrer">
  {#if isValidImage}
    <img class="lp-thumb" src={preview.image_url} alt="" loading="lazy" />
  {/if}
  <div class="lp-body">
    <div class="lp-domain">{preview.domain}</div>
    {#if preview.title}<div class="lp-title">{preview.title}</div>{/if}
    {#if preview.description}<div class="lp-desc">{preview.description}</div>{/if}
  </div>
</a>
{/if}

<style>
  .link-preview {
    display: flex;
    gap: 10px;
    padding: 8px 10px;
    border-left: 3px solid var(--accent, #89b4fa);
    background: var(--bg-sub, rgba(0, 0, 0, 0.2));
    border-radius: 0 6px 6px 0;
    text-decoration: none;
    color: inherit;
    margin-top: 6px;
    max-width: 340px;
    overflow: hidden;
    transition: background 0.15s;
  }
  .link-preview:hover {
    background: var(--bg-hover, rgba(137, 180, 250, 0.08));
  }
  .lp-thumb {
    width: 72px;
    height: 72px;
    object-fit: cover;
    border-radius: 4px;
    flex-shrink: 0;
  }
  .lp-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: hidden;
    min-width: 0;
  }
  .lp-domain {
    font-size: 10px;
    color: var(--accent, #89b4fa);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .lp-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--text, #cdd6f4);
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .lp-desc {
    font-size: 11px;
    color: var(--text-muted, #9399b2);
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }
</style>
