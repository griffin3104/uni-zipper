<script lang="ts">
  // 画面に出すダイアログ(同名ファイルの確認、エラー)。`app.dialog` に入ったものを表示する。
  import { app } from "./app.svelte";
  import Button from "./Button.svelte";
  import Modal from "./Modal.svelte";

  const dialog = $derived(app.dialog);
</script>

{#if dialog?.kind === "conflict"}
  {@const d = dialog}
  <Modal title="同じ名前があります" oncancel={() => d.resolve("cancel")}>
    {#snippet children()}
      <p class="lead">次の名前は、すでに一覧にあるか、ドロップしたものの中で重なっています。</p>
      <ul class="names">
        {#each d.conflicts as name (name)}
          <li>{name}</li>
        {/each}
      </ul>
      <p class="note">選んだ扱いを、これらすべてに適用します。</p>
    {/snippet}
    {#snippet actions()}
      <Button variant="danger" onclick={() => d.resolve("overwrite")}>上書き</Button>
      <Button variant="primary" autofocus onclick={() => d.resolve("rename")}>リネーム</Button>
      <Button onclick={() => d.resolve("cancel")}>キャンセル</Button>
    {/snippet}
  </Modal>
{:else if dialog?.kind === "error"}
  {@const d = dialog}
  <Modal title="エラー" oncancel={() => d.resolve()}>
    {#snippet children()}
      <p class="message">{d.message}</p>
    {/snippet}
    {#snippet actions()}
      <Button variant="primary" autofocus onclick={() => d.resolve()}>OK</Button>
    {/snippet}
  </Modal>
{/if}

<style>
  p {
    margin: 0 0 8px;
  }

  .note {
    color: var(--text-muted);
    font-size: 13px;
  }

  .message {
    word-break: break-all;
  }

  .names {
    max-height: 140px;
    margin: 0 0 8px;
    padding: 8px 12px 8px 28px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    word-break: break-all;
  }
</style>
