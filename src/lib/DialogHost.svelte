<script lang="ts">
  // 画面に出すダイアログ(同名ファイルの確認、エラー)。`app.dialog` に入ったものを表示する。
  import { app } from "./app.svelte";
  import Button from "./Button.svelte";
  import CharsetDialog from "./CharsetDialog.svelte";
  import Modal from "./Modal.svelte";
  import PasswordDialog from "./PasswordDialog.svelte";

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
{:else if dialog?.kind === "confirm"}
  {@const d = dialog}
  <Modal title={d.title} oncancel={() => d.resolve(false)}>
    {#snippet children()}
      <p class="message">{d.message}</p>
    {/snippet}
    {#snippet actions()}
      <Button variant="danger" onclick={() => d.resolve(true)}>{d.okLabel}</Button>
      <Button variant="primary" autofocus onclick={() => d.resolve(false)}>キャンセル</Button>
    {/snippet}
  </Modal>
{:else if dialog?.kind === "password"}
  {@const d = dialog}
  <PasswordDialog title={d.title} message={d.message} resolve={d.resolve} />
{:else if dialog?.kind === "charset"}
  {@const d = dialog}
  <CharsetDialog preview={d.preview} neitherValid={d.neitherValid} resolve={d.resolve} />
{:else if dialog?.kind === "done"}
  {@const d = dialog}
  <Modal title={d.title} oncancel={() => d.resolve()}>
    {#snippet children()}
      <p class="message">{d.message}</p>
      {#if d.skipped.length > 0}
        <h3>スキップしたファイル({d.skipped.length}件)</h3>
        <ul class="names">
          {#each d.skipped as s, i (i)}
            <li>{s.name}<span class="reason">{s.reason}</span></li>
          {/each}
        </ul>
      {/if}
      {#if d.renamed.length > 0}
        <h3>名前を変更したファイル({d.renamed.length}件)</h3>
        <ul class="names">
          {#each d.renamed as r, i (i)}
            <li>{r.from}<span class="arrow"> → </span>{r.to}</li>
          {/each}
        </ul>
      {/if}
    {/snippet}
    {#snippet actions()}
      <Button variant="primary" autofocus onclick={() => d.resolve()}>OK</Button>
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

  h3 {
    margin: 12px 0 4px;
    font-size: 13px;
  }

  .reason {
    display: block;
    color: var(--text-muted);
    font-size: 12px;
  }

  .arrow {
    color: var(--text-muted);
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
