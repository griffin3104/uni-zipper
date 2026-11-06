<script lang="ts">
  // 圧縮モードのファイル一覧。ワークフォルダ直下の項目を並べ、1件ずつ削除できる。
  import { app } from "./app.svelte";
  import Icon from "./Icon.svelte";
</script>

<ul>
  {#each app.items as item (item.name)}
    <li class:zip={item.isZip}>
      <span class="icon"><Icon name={item.isDir ? "folder" : item.isZip ? "zip" : "file"} /></span>
      <span class="name" title={item.name}>{item.name}</span>
      {#if item.isZip}<span class="tag">Zip</span>{/if}
      <button
        class="remove"
        aria-label="{item.name}を一覧から削除"
        title="削除"
        disabled={app.busy}
        onclick={() => app.removeItem(item.name)}
      >
        <Icon name="close" size={16} />
      </button>
    </li>
  {/each}
</ul>

<style>
  ul {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 4px;
    overflow-y: auto;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
  }

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 6px 6px 10px;
    border-radius: 8px;
  }

  li:hover {
    background: var(--surface-hover);
  }

  /* Zipは、ほかと色を変える */
  li.zip {
    background: var(--zip-soft);
  }

  .icon {
    display: flex;
    color: var(--text-muted);
  }

  .zip .icon,
  .zip .tag {
    color: var(--zip);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tag {
    font-size: 11px;
    font-weight: 700;
  }

  .remove {
    display: flex;
    padding: 4px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  .remove:hover:not(:disabled) {
    background: var(--border);
    color: var(--danger);
  }

  .remove:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
