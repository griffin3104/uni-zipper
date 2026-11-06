<script lang="ts">
  // 線画のアイコン(24×24)。色は `currentColor` で、親の `color` に従う。
  type Name = "file" | "folder" | "zip" | "lock" | "close" | "drop";
  let { name, size = 20 }: { name: Name; size?: number } = $props();

  const paths: Record<Name, string> = {
    file: "M7 3h7l5 5v13H7zM14 3v5h5",
    folder: "M3 6h6l2 2h10v11H3z",
    zip: "M7 3h7l5 5v13H7zM14 3v5h5M11 11h2M11 14h2M11 17h2",
    lock: "M6 11h12v9H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3",
    close: "M6 6l12 12M18 6L6 18",
    drop: "M12 4v10M8 10l4 4 4-4M5 19h14",
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={paths[name]} />
</svg>
