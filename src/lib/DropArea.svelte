<script lang="ts">
  import Icon from "./Icon.svelte";

  let { active = false }: { active?: boolean } = $props();
</script>

<div class="drop" class:active>
  <Icon name="drop" size={44} />
  <p class="title">ここにファイル・フォルダ・Zipをドラッグ&ドロップ</p>
  <p class="hint">Zipを1つだけ入れると解凍、それ以外は圧縮します</p>
</div>

<style>
  .drop {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: 16px;
    border: 2px dashed var(--border);
    border-radius: 16px;
    color: var(--text-muted);
    text-align: center;
    transition:
      border-color 0.15s,
      background-color 0.15s,
      color 0.15s;
  }

  .drop.active {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }

  p {
    margin: 0;
  }

  .title {
    margin-top: 8px;
    color: var(--text);
    font-size: 15px;
    font-weight: 600;
  }

  .hint {
    font-size: 12px;
  }
</style>
