<script lang="ts">
  // パスワード入力ダイアログ。開くたびに部品が作られるので、入力欄は空から始まる。
  import Button from "./Button.svelte";
  import Modal from "./Modal.svelte";

  let {
    title,
    message,
    resolve,
  }: {
    title: string;
    message: string;
    resolve: (password: string | null) => void;
  } = $props();

  let password = $state("");
</script>

<Modal {title} oncancel={() => resolve(null)}>
  {#snippet children()}
    <form
      id="password-form"
      onsubmit={(e) => {
        e.preventDefault();
        resolve(password);
      }}
    >
      <p>{message}</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input type="password" autocomplete="off" autofocus aria-label="パスワード" bind:value={password} />
    </form>
  {/snippet}
  {#snippet actions()}
    <Button onclick={() => resolve(null)}>キャンセル</Button>
    <Button variant="primary" submit form="password-form" onclick={() => {}}>OK</Button>
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 8px;
  }

  input {
    width: 100%;
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    user-select: text;
    -webkit-user-select: text;
  }

  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }
</style>
