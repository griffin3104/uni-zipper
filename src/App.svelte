<script lang="ts">
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { onMount } from "svelte";
  import { app } from "./lib/app.svelte";
  import Button from "./lib/Button.svelte";
  import DialogHost from "./lib/DialogHost.svelte";
  import DropArea from "./lib/DropArea.svelte";
  import ExtractPanel from "./lib/ExtractPanel.svelte";
  import ItemList from "./lib/ItemList.svelte";

  onMount(() => {
    app.init();

    // ファイルのドロップは、HTML5のイベントでは実際のパスが取れないので、Tauriのイベントで受け取る。
    let disposed = false;
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          app.dragging = app.acceptsDrop;
        } else if (payload.type === "drop") {
          app.dragging = false;
          app.handleDrop(payload.paths);
        } else {
          app.dragging = false;
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Tauriの外(ブラウザだけで開いたとき)では、ドロップを受け取れない
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  });
</script>

<main class:dragging={app.dragging}>
  {#if app.mode === "initial"}
    <DropArea active={app.dragging} />
  {:else}
    <section class="content">
      {#if app.mode === "compress"}
        <ItemList />
        <p class="hint">ここにドロップして、さらに追加できます</p>
      {:else}
        <ExtractPanel />
      {/if}
    </section>
    <footer>
      <Button disabled={app.busy} onclick={() => app.clear()}>クリア</Button>
      <span class="spacer"></span>
      {#if app.mode === "compress"}
        <Button variant="primary" disabled title="圧縮は、次の段階で実装します" onclick={() => {}}>保存…</Button>
      {:else}
        <Button variant="primary" disabled title="解凍は、次の段階で実装します" onclick={() => {}}>解凍</Button>
      {/if}
    </footer>
  {/if}
</main>

<DialogHost />

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    outline: 3px solid transparent;
    outline-offset: -3px;
    transition: outline-color 0.15s;
  }

  main.dragging {
    outline-color: var(--accent);
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
    padding: 16px 16px 0;
  }

  .hint {
    margin: 0;
    color: var(--text-muted);
    font-size: 12px;
    text-align: center;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px 16px;
  }

  .spacer {
    flex: 1;
  }
</style>
