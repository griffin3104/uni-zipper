<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    variant = "default",
    disabled = false,
    title,
    autofocus = false,
    onclick,
    children,
  }: {
    variant?: "default" | "primary" | "danger";
    disabled?: boolean;
    title?: string;
    autofocus?: boolean;
    onclick: () => void;
    children: Snippet;
  } = $props();
</script>

<!-- svelte-ignore a11y_autofocus -->
<button class={variant} {disabled} {title} {autofocus} {onclick}>
  {@render children()}
</button>

<style>
  button {
    min-width: 88px;
    padding: 7px 16px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  button:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .primary {
    border-color: var(--accent);
    background: var(--accent);
    color: light-dark(#fff, #10131a);
    font-weight: 600;
  }

  .primary:hover:not(:disabled) {
    background: var(--accent);
    filter: brightness(1.08);
  }

  .danger {
    color: var(--danger);
  }
</style>
