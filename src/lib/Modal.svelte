<script lang="ts">
  // アプリ内のモーダル。標準の `<dialog>` を使うので、Escや背後の操作の遮断、フォーカスの閉じ込めはブラウザが行う。
  // 部品が表示されている間だけ開く(画面から消えれば閉じる)。
  import type { Snippet } from "svelte";

  let {
    title,
    oncancel,
    children,
    actions,
  }: {
    title: string;
    /** Escキーが押された。 */
    oncancel: () => void;
    children: Snippet;
    actions: Snippet;
  } = $props();

  let el: HTMLDialogElement;

  $effect(() => {
    el.showModal();
  });
</script>

<dialog
  bind:this={el}
  aria-labelledby="modal-title"
  oncancel={(e) => {
    e.preventDefault();
    oncancel();
  }}
>
  <h2 id="modal-title">{title}</h2>
  <div class="body">{@render children()}</div>
  <div class="actions">{@render actions()}</div>
</dialog>

<style>
  dialog {
    width: min(420px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    padding: 20px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.3);
  }

  dialog[open] {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  dialog::backdrop {
    background: var(--overlay);
  }

  h2 {
    margin: 0;
    font-size: 16px;
  }

  .body {
    overflow-y: auto;
    user-select: text;
    -webkit-user-select: text;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
