<script lang="ts">
  // ファイル名の文字コードを選ばせるダイアログ。同じ名前を、2通りの読み方で並べて見せる。
  import Button from "./Button.svelte";
  import Modal from "./Modal.svelte";
  import type { Charset, PreviewRow } from "./types";

  let {
    preview,
    neitherValid,
    resolve,
  }: {
    preview: PreviewRow[];
    neitherValid: boolean;
    resolve: (charset: Charset | null) => void;
  } = $props();
</script>

<Modal title="ファイル名の文字コードを選んでください" oncancel={() => resolve(null)}>
  {#snippet children()}
    <p class="lead">文字コードを自動で判定できませんでした。ファイル名が正しく読めるほうを選んでください。</p>
    {#if neitherValid}
      <p class="warn">どちらで読んでも、正しい文字コードとしては読めませんでした。文字化けするかもしれません。</p>
    {/if}
    <table>
      <thead>
        <tr>
          <th>Shift_JISとして読んだ場合</th>
          <th>UTF-8として読んだ場合</th>
        </tr>
      </thead>
      <tbody>
        {#each preview as row, i (i)}
          <tr>
            <td>{row.asShiftJis}</td>
            <td>{row.asUtf8}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/snippet}
  {#snippet actions()}
    <Button onclick={() => resolve(null)}>キャンセル</Button>
    <Button variant="primary" autofocus onclick={() => resolve("shiftJis")}>Shift_JISで解凍</Button>
    <Button variant="primary" onclick={() => resolve("utf8")}>UTF-8で解凍</Button>
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 8px;
  }

  .warn {
    color: var(--danger);
    font-size: 13px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    font-size: 13px;
  }

  th,
  td {
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    text-align: left;
    vertical-align: top;
    word-break: break-all;
  }

  th {
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
  }

  th + th,
  td + td {
    border-left: 1px solid var(--border);
  }

  tr:last-child td {
    border-bottom: none;
  }
</style>
