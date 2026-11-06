<script lang="ts">
  // 圧縮モードの設定: 圧縮先OS、暗号方式、パスワード。入力欄は常に表示する(画面仕様 1章)。
  import { app } from "./app.svelte";
  import Segmented from "./Segmented.svelte";
  import type { EncryptionMethod, TargetOs } from "./types";

  const targets: { value: TargetOs; label: string }[] = [
    { value: "auto", label: "自動" },
    { value: "windows", label: "Windows" },
  ];
  const methods: { value: EncryptionMethod; label: string }[] = [
    { value: "zipCrypto", label: "ZipCrypto" },
    { value: "aes256", label: "AES-256" },
  ];
</script>

<div class="options">
  <div class="row">
    <Segmented legend="圧縮先OS" options={targets} bind:value={app.target} disabled={app.busy} />
  </div>
  <div class="row">
    <Segmented legend="暗号方式" options={methods} bind:value={app.method} disabled={app.busy} />
  </div>
  <label class="row">
    <span class="legend">パスワード</span>
    <input
      type="password"
      placeholder="未入力ならパスワードなし"
      autocomplete="off"
      bind:value={app.password}
      disabled={app.busy}
    />
  </label>
</div>

<style>
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0;
  }

  .row > :global(.segments) {
    flex: 1;
  }

  .legend {
    width: 72px;
    color: var(--text-muted);
    font-size: 12px;
  }

  input {
    flex: 1;
    min-width: 0;
    padding: 5px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    user-select: text;
    -webkit-user-select: text;
  }

  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  input:disabled {
    opacity: 0.5;
  }
</style>
