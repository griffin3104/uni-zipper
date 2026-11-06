<script lang="ts">
  // 解凍モード: Zip名と、パスワード付きの表示。
  import { app } from "./app.svelte";
  import Icon from "./Icon.svelte";

  const target = $derived(app.extractTarget);
</script>

{#if target}
  <div class="panel">
    <div class="zip-icon"><Icon name="zip" size={48} /></div>
    <p class="name" title={target.zipName}>{target.zipName}</p>
    {#if target.inspection.encrypted}
      <p class="badge"><Icon name="lock" size={14} /> パスワード付き</p>
    {/if}
    <p class="meta">{target.inspection.entryCount} 件</p>
    {#if target.inspection.detection.kind === "ambiguous"}
      <p class="meta">ファイル名の文字コードは、解凍時に確認します</p>
    {/if}
  </div>
{/if}

<style>
  .panel {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    text-align: center;
  }

  p {
    margin: 0;
  }

  .zip-icon {
    color: var(--zip);
  }

  .name {
    max-width: 90%;
    overflow: hidden;
    font-size: 16px;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 10px;
    border-radius: 999px;
    background: var(--zip-soft);
    color: var(--zip);
    font-size: 12px;
    font-weight: 600;
  }

  .meta {
    color: var(--text-muted);
    font-size: 12px;
  }
</style>
