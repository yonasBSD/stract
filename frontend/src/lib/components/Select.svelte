<script lang="ts" generics="T">
  import { createEventDispatcher } from 'svelte';

  // eslint-disable-next-line no-undef
  type Option = { value: T; label: string; title?: string };

  interface $$Props extends Omit<Partial<HTMLSelectElement>, 'form' | 'value' | 'options'> {
    class?: string;
    value: T; // eslint-disable-line no-undef
    form?: string;
    submitOnChange?: boolean;
    options: Option[];
    ariaLabel?: string;
    id?: string;
  }

  export let value: T; // eslint-disable-line no-undef
  export let form: string | undefined = void 0;
  export let submitOnChange = false;
  export let options: $$Props['options'];
  export let ariaLabel: string | undefined = undefined;
  export let id: string | undefined = undefined;
  // eslint-disable-next-line no-undef
  const dispatch = createEventDispatcher<{ change: T }>();
</script>

<div>
  <select
    {...$$restProps}
    {form}
    aria-label={ariaLabel}
    {id}
    on:change={(e) => {
      if (!e?.target || !(e.target instanceof HTMLSelectElement)) return;

      const chosen = options[e.target.options.selectedIndex];
      value = chosen.value;
      dispatch('change', chosen.value);

      if (submitOnChange) e.target.form?.submit();
    }}
  >
    {#each options as option}
      <option value={option.value} selected={option.value == value} title={option.title}
        >{option.label}</option
      >
    {/each}
  </select>
</div>

<style lang="postcss">
  select {
    @apply rounded border-none bg-base-100 bg-transparent py-0 text-right;
  }
  option {
    @apply bg-base-100 text-neutral-focus;
  }
</style>
