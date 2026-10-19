<script>
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  import Clock from '@lucide/svelte/icons/clock';
  import CalendarClock from '@lucide/svelte/icons/calendar-clock';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleX from '@lucide/svelte/icons/circle-x';
  import CircleSlash from '@lucide/svelte/icons/circle-slash';
  import Eye from '@lucide/svelte/icons/eye';

  let { job, size = 18 } = $props();
</script>

{#if job.status === 'running'}
  <LoaderCircle {size} class="spin" color="var(--accent)" />
{:else if job.status === 'queued' || job.status === 'pending'}
  <Clock {size} color="var(--muted)" />
{:else if job.status === 'scheduled'}
  <CalendarClock {size} color="var(--accent)" />
{:else if job.status === 'completed' && job.dry_run}
  <Eye {size} color="var(--accent)" />
{:else if job.status === 'completed'}
  <CircleCheck {size} color="var(--success)" />
{:else if job.status === 'failed'}
  <CircleX {size} color="var(--danger)" />
{:else}
  <CircleSlash {size} color="var(--muted)" />
{/if}
