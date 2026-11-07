<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { onMount } from "svelte";
  import { app } from "./lib/app.svelte";
  import Button from "./lib/Button.svelte";
  import CompressOptions from "./lib/CompressOptions.svelte";
  import DialogHost from "./lib/DialogHost.svelte";
  import DropArea from "./lib/DropArea.svelte";
  import ExtractPanel from "./lib/ExtractPanel.svelte";
  import ItemList from "./lib/ItemList.svelte";
  import ProgressView from "./lib/ProgressView.svelte";

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    // 起動時に、「プログラムから開く」で渡されたファイルがあれば、それを受け取る(一覧の取り込みのあとで)。
    app.init().then(() => app.handleOpenRequests());

    // 起動中に、「プログラムから開く」が来たとき(macOS)。
    let unlistenOpen: (() => void) | undefined;
    listen("open-paths", () => app.handleOpenRequests())
      .then((fn) => {
        if (disposed) fn();
        else unlistenOpen = fn;
      })
      .catch(() => {
        // Tauriの外(ブラウザだけで開いたとき)では、受け取れない
      });

    // ファイルのドロップは、HTML5のイベントでは実際のパスが取れないので、Tauriのイベントで受け取る。
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
      unlistenOpen?.();
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
        <CompressOptions />
        <p class="hint">
          {#if app.method === "aes256" && app.password !== ""}
            AES-256は、macOS標準では開けません(Keka、7-Zipなどが必要です)
          {:else}
            ここにドロップして、さらに追加できます
          {/if}
        </p>
      {:else if app.mode === "working"}
        <ProgressView />
      {:else}
        <ExtractPanel />
      {/if}
    </section>
    <footer>
      {#if app.mode === "working"}
        <span class="spacer"></span>
        <Button onclick={() => app.cancelWork()}>キャンセル</Button>
      {:else}
        <Button disabled={app.busy} onclick={() => app.clear()}>クリア</Button>
        <span class="spacer"></span>
        {#if app.mode === "compress"}
          <Button variant="primary" disabled={app.busy} onclick={() => app.startCompress()}>保存…</Button>
        {:else}
          <Button variant="primary" disabled={app.busy} onclick={() => app.startExtract()}>解凍</Button>
        {/if}
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
