<script lang="ts">
  // 処理中: プログレスバーだけを表示する(ファイル名は出さない)。
  import { app } from "./app.svelte";

  // 全体の大きさが分からないときは、`value` を付けず、動き続ける表示にする。
  const fraction = $derived(app.fraction);
</script>

<div class="panel" role="status" aria-live="polite">
  <p class="label">{app.workLabel}</p>
  <progress value={fraction ?? undefined} max="1"></progress>
</div>

<style>
  .panel {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
  }

  .label {
    margin: 0;
    color: var(--text-muted);
  }

  progress {
    width: 70%;
    height: 10px;
    appearance: none;
    border: none;
    border-radius: 999px;
    background: var(--border);
    overflow: hidden;
  }

  progress::-webkit-progress-bar {
    background: var(--border);
  }

  progress::-webkit-progress-value {
    border-radius: 999px;
    background: var(--accent);
    transition: width 0.1s linear;
  }

  progress::-moz-progress-bar {
    background: var(--accent);
  }
</style>
