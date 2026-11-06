<script lang="ts" generics="T extends string">
  // 2〜3個の選択肢から1つ選ぶ、横並びのラジオボタン(見た目はセグメント)。
  let {
    legend,
    options,
    value = $bindable(),
    disabled = false,
  }: {
    legend: string;
    options: { value: T; label: string }[];
    value: T;
    disabled?: boolean;
  } = $props();

  const group = $props.id();
</script>

<fieldset {disabled}>
  <legend>{legend}</legend>
  <div class="segments">
    {#each options as option (option.value)}
      <label class:selected={value === option.value}>
        <input type="radio" name={group} value={option.value} bind:group={value} />
        <span>{option.label}</span>
      </label>
    {/each}
  </div>
</fieldset>

<style>
  fieldset {
    display: contents;
  }

  legend {
    float: left;
    width: 72px;
    padding: 0;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 30px;
  }

  .segments {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    overflow: hidden;
  }

  label {
    position: relative;
    flex: 1;
    padding: 4px 10px;
    cursor: pointer;
    font-size: 13px;
    text-align: center;
  }

  label + label {
    border-left: 1px solid var(--border);
  }

  label.selected {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }

  /* ラジオボタン自体は隠し、キーボード操作(矢印キー)は残す */
  input {
    position: absolute;
    opacity: 0;
    inset: 0;
    cursor: inherit;
  }

  label:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  fieldset:disabled label {
    opacity: 0.5;
    cursor: default;
  }
</style>
